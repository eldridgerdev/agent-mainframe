# Remote Control — companion app

- **Status:** Phases 1–3 implemented (2026-09-28) as a **PWA** served by
  AMF itself, which replaced the Flutter client at the user's direction
  (2026-09-26). It covers status, push, the terminal (simple and full), and
  feature/session/TODO/prompt/diff actions. User guide:
  [`docs/remote-control.md`](../remote-control.md). Detailed progress is in the
  feature's `AMF_PLAN.md`. The Flutter epics below (3, 6) are superseded;
  the `mobile/` Flutter project was removed before merge (2026-09-28).
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
  automatically whenever AMF is running. Asking to pair (`Ctrl+Space Q`)
  while it is off counts as such a request: it starts the server and
  opens the pairing dialog once it is listening. Nothing starts it
  without a keypress.
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
- **Client (superseded 2026-09-26 — now a PWA, see Status)**: a
  cross-platform native app built with **Flutter** (iOS + Android from one
  Dart codebase) — not a PWA. Chosen over a PWA for
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

- [x] Add tokio + axum (or equivalent) dependency.
- [x] Dedicated server thread, started/stopped by the on/off toggle.
- [x] `mpsc` channel wiring: remote requests marshalled onto the main
      loop, responses/state pushed back the same way.
- [x] Toggle surfaced in the dashboard/leader menu (state only — LAN vs.
      tunnel indicator comes with the tunnel work in Epic 4/9).

**Done (2026-09-14).** `src/remote_server.rs` runs the server on a
dedicated `amf-remote-server` thread with its own tokio runtime and a
single `/health` route (real routes land with Epics 4/5/8/9). Shutdown
is a tokio oneshot signal with graceful `axum::serve` teardown;
lifecycle is reported back over a plain `std::sync::mpsc` channel
(`Started`/`Stopped`), matching the existing `ipc.rs` cross-thread
pattern rather than inventing a new one. `App::remote_server` owns the
handle, `App::toggle_remote_server` starts/stops it (bound to
`Ctrl+Space C` on the dashboard — no auto-start, ever; the later
`Ctrl+Space Q` pairing start is a user request too, see "Server
lifecycle" above), and
`App::poll_remote_server_bg` drains events every main-loop tick.
Loopback-only (`127.0.0.1`) on an OS-assigned port at the time.
Correction (2026-09-14, once Epic 4 landed): auth existing doesn't by
itself open up LAN/tunnel exposure — the bind address is unchanged by
Epic 4; widening it is separate follow-up work. **Superseded
(2026-09-28):** the server now binds the configurable
`AppConfig::remote_bind`, `127.0.0.1:47800` by default — a fixed port so
a tunnel survives restarts (see `docs/remote-control.md`). Only tests
still bind port 0.

Verification: 4 new tests (start/stop without a client, drop-without-
explicit-stop joins cleanly and doesn't hang, two servers on
independent OS-assigned ports, full toggle round-trip through `App`).
Full suite (2251 tests), `cargo clippy --all-targets -- -D warnings`,
and `cargo fmt --check` all pass.

### Epic 2 — Device storage (P0)

Fully independent of Epic 1 — this is a data-model addition only.
Safe to build first or in parallel.

- [x] `remote_devices` migration, following the existing `MIGRATION_0xx`
      convention.
- [x] `src/db/remote_devices.rs` (or similar): create, lookup by token,
      revoke, update last-seen.

**Done (2026-09-14).** `MIGRATION_042` (numbered 029 when first written,
renumbered by later merges of main) adds `remote_devices` (id, name,
token_hash, paired_at, last_seen_at, revoked) with a UNIQUE index on
`token_hash`. `src/db/remote_devices.rs` stores only the hashed token —
minting and hashing a real token is Epic 4's job — and exposes
create/find-by-id/find-by-token-hash/list-all/touch-last-seen/revoke,
wrapped as `AmfDb` methods. Originally marked `#[allow(dead_code)]`
until Epic 4 called in; that allow is gone now that pairing, auth, and
revoke use the module, and the two lookups only tests use
(find-by-id/find-by-token-hash) are `#[cfg(test)]`. Web Push's tables are
`MIGRATION_043`.

Verification: 7 new unit tests (create/lookup by id/lookup by token
hash/unknown lookups return `None` not an error/revoke leaves the row
findable so callers can distinguish unknown-vs-revoked/touch-last-seen/
list-all/unique-token-hash constraint). Full suite (2258 tests, after
also bumping 4 migration tests that hardcoded the prior latest-version
number), clippy, and fmt all pass.

### Epic 3 — Native app groundwork (P0)

Front-loads the app-store-adjacent lead time the Flutter decision adds
(account approval, signing, CI) so it isn't discovered as a blocker
mid-Phase-1. Fully independent of Epics 1–2; can start immediately.
**Narrowed (2026-09-14) to Android-only for now** — iOS (Apple Developer
account, TestFlight, code signing) is deliberately deferred, per the user.

- [x] Flutter project scaffold (Android target only; iOS deferred).
- [ ] ~~Apple Developer account~~ — deferred with iOS.
- [ ] Google Play Console account (or confirm an existing one can be
      used). Still open — not needed for local `adb install` testing.
- [ ] Code signing for Android (a release/upload key). Still open — debug
      builds are unsigned-for-distribution by default and that's all
      that exists so far.
- [ ] Play Console internal-testing track configured for installing dev
      builds on a real phone without a store release. Still open —
      superseded for now by `adb install` (see `mobile/README.md`).
- [x] Documented local build steps producing an installable artifact
      (`mobile/README.md`); CI is still open.

**Removed (2026-09-28).** Superseded by the PWA; the `mobile/` project
described below was deleted before merge rather than kept as dead code.

**Done (2026-09-14), scaffold half.** `mobile/` is a Flutter project
(`flutter create --platforms=android --org dev.agentmainframe
--project-name amf_companion mobile`), Android-only. Toolchain (Flutter
3.47.4 stable + Android SDK platform 36 / build-tools 34 & 36, no
sudo/snap — extracted from the official tarballs into `~/dev/flutter`
and `~/dev/android-sdk`, since this dev box has no root access in this
session) is documented in `mobile/README.md` rather than committed
(machine-local, like any other SDK install). `flutter build apk --debug`
produces `mobile/build/app/outputs/flutter-apk/app-debug.apk`; nested
`mobile/.gitignore` (from the template) keeps `build/`, `.dart_tool/`,
`.idea/`, and `android/local.properties` out of git the same way the
main repo's `target/` is already excluded — only source, `pubspec.*`,
and the Android project skeleton are tracked.

Also built the actual Phase 1 screens on top of the scaffold (a step
ahead of where this epic's checklist originally stopped, since an empty
counter-app scaffold wasn't worth committing on its own): pairing
(`lib/pairing_screen.dart`, manual server-address + code entry —
`POST /pair/exchange`) and status (`lib/status_screen.dart`, polls
`GET /status` every 5s with the stored bearer token, clears the
credential and returns to pairing on a 401). `lib/models.dart` /
`lib/api_client.dart` mirror `src/remote_server.rs`'s JSON shapes
directly; `lib/credential_store.dart` persists the one device credential
via `shared_preferences`. Not built yet: QR-code scanning (manual entry
only — also the desktop dialog's own fallback) and Firebase push
(Epic 6's job, and a separate external-service decision).

Verification: `flutter analyze` and `flutter test` (2 new widget tests —
shows pairing with no stored credential, goes straight to status with
one) both clean; `flutter build apk --debug` succeeds. **Not done:**
install on a real device — no phone was connected in this session (the
server is loopback-only regardless; see `mobile/README.md` for the
`adb reverse` step needed to test pairing for real once one is).

### Epic 4 — Pairing flow (P1)

Needs Epic 1 (server to expose the exchange endpoint) and Epic 2
(device storage).

- [x] One-time pairing code generation + QR rendering on desktop.
- [x] Code exchange endpoint issuing a per-device token.
- [x] Rate-limiting/lockout on repeated failed pairing attempts.
- [x] Pairing dialog UI (QR + code + status) on desktop.

**Done (2026-09-14).** `App::start_pairing` (`Ctrl+Space Q`, dashboard
leader — `C` was already taken for the toggle; checked both the
dashboard- and view-leader namespaces before picking `Q`, free in both)
opens `AppMode::RemotePairing`, whose state *is* the pending pairing —
there's no separate copy on `App`, so closing the dialog invalidates the
code by construction rather than by a second cleanup step. The code is a
6-digit number (`remote_server::generate_pairing_code`, `uuid`-backed —
no new RNG dependency); the QR encodes `amf-pair://<addr>?code=<code>`
via a new `src/qr.rs` (the `qrcode` crate, half-block Unicode rendering,
the same approach sketched for the unrelated session-URL QR in
`remote-control-qr-overlay-plan.md`, landing here first).

`POST /pair/exchange` (`src/remote_server.rs`) never itself decides
whether a code is valid: per the plan's DB-concurrency decision the main
loop is the sole SQLite writer, so the handler only forwards the request
as a `PairingExchangeRequest` and `.await`s the outcome on a `oneshot`
embedded in it (a 5s timeout guards against an unresponsive main loop).
`App::process_pairing_exchange` (`src/app/remote_server.rs`), drained
every tick by `poll_remote_server_bg` alongside the existing lifecycle
events, validates against the dialog's own state: wrong code increments
`attempts` and locks out at `MAX_PAIRING_ATTEMPTS` (5); a *correct* code
locks the same way immediately after minting a device, making the code
single-use rather than replayable for as long as the success screen is
up. Token minting (`generate_device_token`) and hashing
(`hash_token`, SHA-256) live in `remote_server.rs`, called from the App
side that owns the DB write — `db/remote_devices.rs`'s Epic 2 comment
about this being Epic 4's job is now accurate. A stopped server flips an
open dialog to a "server stopped" failure state instead of leaving it
stuck on "Waiting for phone…".

Verification: 18 new tests — token issuance and DB persistence, blank
device-name fallback, wrong-code rejection without burning the real
code, lockout at the attempt cap (and that lockout also blocks the
*correct* code), single-use enforcement after success, expiry,
regenerate replacing the code, no-DB-configured failure, a server-stop
mid-dialog transition, and a real end-to-end HTTP round trip
(`ureq` POST against a live `/pair/exchange`, wrong code then right code,
driven entirely through `App::poll_remote_server_bg` the way a real
phone's requests would be). Full suite (2276 tests), clippy
`--all-targets -D warnings`, and `cargo fmt --check` all clean.
**Not done:** the "one manual pass pairing a real phone" verification
item — there is no phone client yet (Epic 3/6) and the server is still
loopback-only (`127.0.0.1`, see Epic 1), so nothing off this machine can
reach `/pair/exchange` yet regardless. Revisit once Epic 6 (or a LAN/
tunnel bind) exists to actually try it.

### Epic 5 — Status/notification relay (P1)

Needs Epic 1 only — the read-only relay logic can be built and tested
against a local client before pairing/auth exists, though it should be
gated behind auth (Epic 4) before being exposed on a real network.

- [x] Read-only status/notification endpoint (WebSocket or polling)
      sourced from the existing `app/notifications.rs` scan.
- [x] Independent of the remote-control toggle's on/off state, per the
      notification/toggle split in Architecture.
- [x] Auth-gated once Epic 4 lands (do not ship unauthenticated on a
      real network). Done (2026-09-14) as part of Epic 7 — see that
      epic for the middleware and its tests.

**Done (2026-09-14), backend half.** `GET /status` on
`src/remote_server.rs` serves a `RemoteStatusSnapshot` (plain polling,
not a WebSocket — simplest thing that works for a feed this small and
low-frequency; nothing here rules out a push transport later).
`App::build_remote_status_snapshot` builds it from `self.store` plus the
existing in-memory `self.attention` map — attention *detection* was
already independent of this toggle before this epic (that's
`app/notifications.rs`, untouched here); what's new is a read-only
window onto it. Pushed to the server thread over a `tokio::mpsc`
channel every `poll_remote_server_bg` tick; a relay task holds the
latest snapshot behind a `Mutex` so the server thread never reads `App`
directly. `RemoteFeatureStatus` is a deliberately narrow wire type, not
a mirror of `project::Feature`. The Flutter-side status *view* is Epic
6's job — this epic is the backend feed it will call.

Verification: 3 new tests, including a real HTTP round trip
(`ureq::get` against a live `/status`) both before and after
`publish_status`, and the `poll_remote_server_bg` publish loop verified
end-to-end over real HTTP from `App`. Full suite (2261 tests, run twice
for flakiness), clippy, and fmt all clean.

### Epic 6 — App shell + push (P1)

Needs Epic 3 (scaffold) to exist at all; needs Epic 4 for a real pairing
flow and Epic 5 for real status data, though UI scaffolding for both
screens can be built against mocked data in parallel with those landing.

- [x] Pairing/scan screen. Done in the PWA (2026-09-26): the QR opens
      the pairing page with the code filled in, so the phone's own camera
      does the scanning.
- [x] Status/notification list screen (Phase 1 view). Done in the PWA,
      sorted by status within each project (2026-09-28).
- [x] ~~Firebase Cloud Messaging~~ push notifications. Done as Web Push
      with AMF's own VAPID key (2026-09-26/28), so no Firebase project.

Verification: manual install/pairing on a real phone; confirm a
notification triggered by a real agent question arrives. Neither done
yet — no phone was connected this session (see Epic 3's verification
note) and push isn't built.

### Epic 7 — Device revoke (P1)

Needs Epic 4 (pairing/token model).

- [x] Revoke action in the desktop paired-devices list.
- [x] Revoke closes any active connection for that device immediately
      (not just on next reconnect).
- [x] Revoked token rejected on all subsequent requests.

**Done (2026-09-14).** Two halves. First, `/status` actually checks a
token now: `require_device_auth` (`src/remote_server.rs`) is an axum
`route_layer` on `/status` that reads `Authorization: Bearer <token>`
and checks it against an in-memory `HashMap<token_hash,
AuthorizedDevice>`. That table is published by `App` every tick
(`App::build_authorized_devices`, alongside the existing status
snapshot) from `db.list_remote_devices()` with revoked rows filtered
out — same channel-routed shape as the status feed and pairing
exchange, so the server thread still never opens its own database
connection. A hit reports the device id back over a fire-and-forget
channel so `App::drain_device_seen_events` can record `last_seen_at`
from the main loop. Missing, unknown, and revoked tokens all produce
the same 401 (no signal beyond "no", matching the pairing exchange's
own non-disclosure).

Second, the revoke UI itself: `v` from the pairing dialog
(`Ctrl+Space Q`) opens a paired-devices list as a sub-screen of that
same dialog (`RemotePairingState::view: PairingDialogView`) rather than
a new leader binding or `AppMode` — the list only makes sense in
relation to an open pairing session, and `Esc` from it returns to the
pairing screen rather than closing the dialog outright. `j`/`k` move
the cursor; `d`, `d` revokes the selected device (arm on the first
press, confirm on the second, any other key clears the arm) — the same
contract the prompt overrides manager's `d`, `d` clear uses. There is
no live connection for a revoke to tear down yet (that arrives with
Epic 9's terminal streaming); "immediately" for now means the very
next tick's published table excludes the device, which is exactly what
an unknown token already looks like to `require_device_auth`.

Verification: 3 new server-level tests (`/status` rejects no token and
an unrecognized one, a valid one passes and reports last-seen) and 6
new `App`-level tests (open/close the devices view, selection wraps
and clears a pending confirmation, the arm-then-confirm flow updates
both the DB row and the dialog's own copy, any other key clears the
confirmation, a revoked device drops out of the authorized table) —
all pass; full suite (2662 tests), clippy `--all-targets -D warnings`,
and `cargo fmt --check` all clean.

### Epic 8 — Prompt response (P2)

Needs the full Phase 1 stack (Epics 1, 5, 6) — this extends the same
server/app surfaces rather than introducing new ones.

- [x] ~~Endpoint for reading an agent's pending question.~~ /
      ~~Endpoint for submitting a response.~~ Done through the session
      view instead (2026-09-28): the rendered pane plus quick keys and a
      reply box, so no separate question endpoint was needed.
- [x] Prompt-response view — the PWA's Simple session view.

Verification: automated test round-tripping a captured question/answer;
manual test answering a real agent prompt from the phone.

### Epic 9 — Terminal streaming, backend (P3)

Needs Epic 1 (server) and Epic 4 (auth) — full terminal access is the
highest-privilege capability in this plan and must not ship
unauthenticated. Independent of Epic 8 (prompt response); could be
built in parallel with it if capacity allows, though sequencing after
Phase 2 keeps risk ordered from lowest to highest privilege.

- [x] Stream `TmuxManager::capture_pane_ansi` output over WebSocket
      (`src/remote_terminal.rs`, 2026-09-28).
- [x] Forward phone keystrokes back through the existing
      `send_literal`/`send_key_name` paths.
- [x] Shared read/write with local access, no conflict handling
      (matching the concurrent-access decision).

Verification: manual test typing from both phone and desktop into the
same session; automated test on the send/receive framing logic.

### Epic 10 — Client rendering modes (P3)

Needs Epic 6 (app shell) and Epic 9 (terminal backend).

- [x] xterm.js full-terminal view (the PWA's Full view, 2026-09-28).
- [x] Simplified mobile view (the PWA's Simple view).
- [x] User toggle between the two, remembered per device.

Verification: manual check of both views over both LAN and tunnel
connections.

### Real-use follow-ups (2026-09-28)

Found by using it on a real phone over Tailscale:

- [x] Dead `$AMF_BIN` after a rebuild silenced every hook, so nothing
      reached attention or push. Hooks now fall back to `amf` on `PATH`
      (#667).
- [x] `Ctrl+Space C` in a session sent Claude's `/rc` instead of
      toggling AMF's server, and `Q` did nothing there. Both now drive
      AMF Remote from a session too, and the pairing dialog returns to
      the session on close. The `/rc` toggle key was dropped.
- [x] `Ctrl+Space Q` refused while the server was off. It now starts
      the server and opens the dialog once it's listening.
- [x] Without `remote_public_url`, the QR pointed at `127.0.0.1`, which
      no phone can open, with no warning. The dialog now says so.
- [x] Phone home list sorted by status (attention, active, idle,
      stopped).
- [ ] A userspace `tailscaled` doesn't survive a WSL restart, so the
      tunnel silently disappears. This is a user-setup issue, but the docs
      could mention it.
- [ ] Still unverified end to end: a push triggered by a real agent,
      and answering a real Claude permission prompt from the phone.

### PWA dashboard follow-up — collapsible projects

**Implemented (2026-09-28), pending release.** Project headings on the
AMF Remote dashboard now toggle their feature lists and show feature counts.
Projects start expanded; each device remembers collapsed projects across
status refreshes, navigation and reloads. The needs-attention list stays
visible above the project groups.

- [x] Independent collapse/expand controls with touch and keyboard support.
- [x] Preserve collapsed state and keyboard focus during status refreshes.
- [x] Remember collapsed projects on this device, with safe fallbacks when
      browser storage is unavailable or saved preferences are malformed.
- [x] Update the PWA shell cache and the phone companion user guide.

Verification: Chromium interaction checks passed for touch, keyboard,
feature counts, attention visibility, refreshes, navigation, reloads,
narrow layouts and storage fallbacks. The full parallel workspace suite
passed on an isolated tmux socket (2,979 passed; one existing GitHub
acceptance test ignored), as did formatting and strict all-target Clippy.

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
  discover it mid-Phase-1. **Narrowed (2026-09-14):** iOS is deferred
  entirely for now (per the user), so only the Android half of this
  (Play Console account, signing, internal-testing track) is still
  open — see Epic 3.
- **Resolved (2026-09-26): push uses Web Push with AMF's own VAPID
  key, so none of this applies.** Firebase Cloud Messaging (Epic 6's push notifications) needed a
  Firebase project created and wired up (a new external-service
  dependency, `google-services.json` committed or generated per build,
  a server-side key for AMF to send from) — not yet decided or
  scoped; needs sign-off before Epic 6's push half starts.
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
