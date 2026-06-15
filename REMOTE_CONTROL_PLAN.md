# Remote Control Integration Plan

Integrate Claude Code's **Remote Control** feature into AMF so a
feature's local Claude session can be driven from claude.ai/code or
the Claude mobile app, while the agent keeps running on the machine
inside its tmux session.

## 1. What Remote Control is

Remote Control (research preview, Claude Code v2.1.51+) bridges a
**local** Claude Code session to claude.ai/code and the Claude iOS /
Android apps. It is a synchronization layer, not a cloud migration:

- Claude keeps running locally the whole time. The web / mobile UI is
  just a window into the local session.
- The local filesystem, MCP servers, tools, and project config stay
  available; `@` autocompletes local paths.
- The conversation stays in sync across terminal, browser, and phone
  at the same time.
- The session reconnects automatically after a sleep or short network
  drop.
- Connection is outbound HTTPS only (no inbound ports). It registers
  with the Anthropic API and polls for work.

This is distinct from **Claude Code on the web** (runs in Anthropic
cloud infra) and from **Dispatch** (mobile-triggered Desktop session).
Remote Control is the right fit for AMF because AMF already runs each
agent as a long-lived local process in tmux.

### Invocation modes

- `claude remote-control` — standalone **server mode**. Stays running,
  prints a session URL, spacebar shows a QR code, serves up to
  `--capacity` sessions, `--spawn same-dir|worktree|session`.
- `claude --remote-control` (alias `--rc`) — a **normal interactive
  session** with Remote Control enabled. Optional positional name:
  `claude --remote-control "My Project"`. You can still type locally.
- `/remote-control` (alias `/rc`) — enable from **inside** a running
  session, carrying over conversation history.
- `/config` → **Enable Remote Control for all sessions** = `true` —
  global default for every interactive session.

### Indicator and URL

- v2.1.162+: a footer indicator below the input box stays up while
  connected.
- v2.1.172+: the indicator text reads `/rc active` (hidden when the
  terminal is too narrow); earlier versions read `Remote Control
  active`. The indicator is a link to the session on claude.ai.
- Session title precedence: `--name` / `/remote-control` arg →
  `/rename` → last meaningful message → auto name like
  `myhost-graceful-unicorn` (`myhost` = hostname or
  `--remote-control-session-name-prefix`, env
  `CLAUDE_REMOTE_CONTROL_SESSION_NAME_PREFIX`).

### Requirements and guards (important for AMF)

- **Subscription**: Pro / Max / Team / Enterprise. **API keys are not
  supported.** On Team/Enterprise an admin must enable the org toggle.
- **Auth**: must be claude.ai OAuth (`/login`). A `setup-token` /
  `CLAUDE_CODE_OAUTH_TOKEN` "full-scope" token is inference-only and is
  rejected. Third-party providers (`CLAUDE_CODE_USE_BEDROCK`,
  `_VERTEX`, `_FOUNDRY`) are not supported.
- **z.ai relevance**: AMF's `ZaiPlanConfig` points Claude at a
  third-party endpoint. Remote Control will not work for those
  sessions — AMF must detect this and not offer / not enable RC there.
- `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` / `DISABLE_TELEMETRY` can
  fail the eligibility check.
- **Version**: v2.1.51+ minimum; v2.1.172+ for the `/rc active`
  indicator that AMF should parse.

### Limitations to design around

- **One remote session per interactive process** (server mode lifts
  this). AMF's one-Claude-per-window model already matches this.
- **Local process must stay alive** — fine, tmux keeps it alive.
- **~10 min network outage** ends the session; must re-run.
- **Ultraplan disconnects RC** (both occupy claude.ai/code).
- Some commands are local-only (`/plugin`, `/resume`); most text
  commands work from web/mobile.

## 2. Why it fits AMF

AMF launches `claude` interactively inside a persistent tmux window
(`TmuxManager::launch_claude(session, window, resume_id, extra_args)`)
and embeds the pane. Remote Control's interactive mode
(`claude --remote-control`) slots directly into the existing
`extra_args` path with zero new process-management machinery. The
"one remote session per process" limit is already how AMF models a
Claude session, and the "local process must keep running" requirement
is satisfied by tmux.

**Recommendation:** ship interactive mode
(`claude --remote-control "<name>"`) as the primary integration.
Treat server mode as a later, optional "remote server" session kind.

## 3. Where it goes in AMF

The existing `enable_chrome` boolean is the exact template — it is a
per-feature flag that lives on the data model, flows through feature
creation / presets / hooks / automation, and contributes a CLI flag in
`VibeMode::cli_flags`. Remote Control should mirror it as
`remote_control: bool`.

### 3.1 Data model (`src/project.rs`)

- Add `remote_control: bool` to `Feature`
  (`#[serde(default, skip_serializing_if = "is_false")]`).
- Extend the launch-flag builder. `VibeMode::cli_flags(enable_chrome)`
  currently only knows about Chrome. To add `--remote-control` and the
  `--name`, either:
   - change the signature to take a small `LaunchOpts` struct
     (`enable_chrome`, `remote_control`, `session_name`), or
   - keep `cli_flags` for mode/Chrome and append RC flags at the call
     site in `feature_ops.rs` where the feature name is in scope.
  Prefer the `LaunchOpts` struct so every call site
  (`feature_ops.rs:684`, `claude_session_picker.rs:269`,
  `codex_session_picker.rs:279`) stays consistent.
- RC flags only apply to `SessionKind::Claude` launches. Pass
  `--name "<feature/branch name>"` so the claude.ai session list is
  legible (e.g. the branch or feature name).

### 3.2 Feature creation (`src/app/feature_ops.rs`,
`src/app/state.rs`, `src/handlers/feature_creation.rs`,
`src/ui/dialogs/feature.rs`)

- Add `remote_control` to `CreateFeatureState` and thread it through
  the same call chain as `enable_chrome` (see the ~20 `enable_chrome`
  sites in `feature_ops.rs` / `hooks.rs` / `automation.rs`).
- Add a toggle to the feature-creation wizard next to the existing
  Chrome / plan / review toggles.
- Guard the toggle: if the resolved auth is API-key / z.ai / a
  third-party provider, show it disabled with a one-line reason rather
  than letting the user enable something that will error.

### 3.3 Presets (`src/extension.rs`, `amf-add-preset` skill)

- Add `remote_control: bool` to `FeaturePreset` and
  `FeaturePresetDe` (defaults `false`).
- Update the `amf-add-preset` skill so a preset can pre-enable Remote
  Control, alongside the existing harness / mode / Chrome options.

### 3.4 Config default (`src/app/mod.rs` `AppConfig`)

- Add an optional global default (e.g.
  `remote_control_default: bool`) so users who always want RC for new
  Claude features get it without per-feature clicks. This mirrors
  Claude Code's own `/config` "Enable for all sessions", but scoped to
  AMF feature creation so AMF stays the source of truth.

### 3.5 Runtime toggle (`src/app/view.rs`, `src/ui/pane.rs`
`draw_leader_menu`, `src/handlers/view.rs`)

- Add a leader-menu entry, e.g. `leader+R` "Toggle Remote Control",
  that sends `/rc` + Enter to the focused Claude pane (reuse the
  existing key-send path; mind the compose-input interception — route
  it like the other leader commands so it does not get captured by the
  AMF composer).
- After toggling, AMF scans the pane for the indicator / URL to update
  state (see 3.6).

### 3.6 Surfacing the session URL / QR

The embedded TUI cannot click the footer link, so AMF should detect
and surface it:

- **Detect**: capture the pane (existing `capture_pane` /
  vt100 path) and scan for the `/rc active` (or `Remote Control
  active`) indicator and a `claude.ai/code` session URL.
- **Copy**: a leader command to copy the URL to the clipboard via the
  existing `crate::app::util::copy_to_clipboard` (wl-copy/pbcopy), with
  a success toast — same UX as `copy_selected_prompt_to_clipboard`.
- **Open**: optional "open in browser" (`xdg-open` / `open`).
- **QR (stretch)**: render a QR of the URL in a TUI overlay using
  half-block glyphs for phone scanning, mirroring server mode's
  spacebar QR.

### 3.7 Dashboard indicator (`src/ui/dashboard.rs`)

- Show a per-session badge when RC is active (and online vs offline),
  similar to the existing `[direct input — leader+e: compose]` badge at
  `dashboard.rs:771`. E.g. `[remote ●]` green when connected.

### 3.8 Preconditions / detection (`src/claude.rs`)

- **Version check**: extend `ClaudeLauncher` to read `claude
  --version` and gate RC behind v2.1.51+ (prefer v2.1.172+ for the
  parseable indicator). Below that, hide/disable with a hint to
  upgrade.
- **Auth/eligibility check**: detect API-key / z.ai / Bedrock / Vertex
  / Foundry configurations and disable RC with the specific reason,
  rather than launching a session that errors with "Remote Control
  requires a claude.ai subscription".

## 4. How it should work (UX)

1. **Create a feature with RC on**: user enables the Remote Control
   toggle (or it is on via preset / global default). AMF launches
   `claude --remote-control "<feature name>"` in the tmux window.
2. **Get the link**: AMF detects the session URL from the pane;
   `leader` menu offers Copy URL / Open in browser / Show QR. The
   dashboard shows a `[remote ●]` badge.
3. **Drive from phone/web**: user opens the URL or scans the QR, picks
   the session up on claude.ai/code or the Claude app, and messages
   sync back into the embedded AMF pane live.
4. **Toggle at runtime**: `leader+R` enables/disables RC on the
   focused session by sending `/rc`.
5. **Push notifications**: note in docs that with RC active Claude can
   push to the phone when a long task finishes (`/config` → "Push when
   Claude decides"); AMF does not need to implement this, just surface
   it.

## 5. Edge cases to handle

- z.ai / API-key / third-party provider sessions: never offer RC.
- Ultraplan started in a session disconnects RC — expect the badge to
  drop; do not treat as an error.
- 10-minute network outage ends RC; AMF should detect the indicator
  disappearing and clear the badge, optionally offering re-enable.
- Resumed sessions (`--resume`): decide whether to re-add
  `--remote-control` on resume (yes, if the feature flag is on).
- Multiple Claude sessions in one feature: each is its own process →
  each can have its own RC session; names should disambiguate.

## 6. Phased implementation

1. **Phase 1 — launch flag (MVP)** — ✅ **done**. `remote_control` on
   `Feature` + `LaunchOpts`, wizard toggle (Claude-only, focus 5;
   steering moved to focus 6), presets, and the availability guard.
   Launches `claude --remote-control "<name>"`. No URL surfacing yet
   (user reads the footer in the embedded pane). Guard details:
   - z.ai / third-party provider (Bedrock / Vertex / Foundry, via env)
     and Claude Code < v2.1.51 are detected by
     `ClaudeLauncher::remote_control_block_reason(zai_configured)`.
   - The wizard stores `remote_control_available` +
     `remote_control_block_reason`; when blocked the toggle is inert and
     renders the specific reason instead of a checkbox.
   - All launch sites gate `--remote-control` behind
     `App::remote_control_allowed()` so the flag is never passed to an
     incompatible session.
   - `ANTHROPIC_API_KEY` presence is intentionally *not* a hard block
     (claude.ai OAuth can coexist and takes precedence); Claude surfaces
     its own error if the session truly resolves to an API key.
2. **Phase 2 — surfacing** — ✅ **done**. Pane detection of the
   indicator/URL, copy + open leader commands, dashboard badges.
   Details:
   - `app::remote_control::detect_remote_control(pane)` strips ANSI/OSC
     escapes and returns `{ active, url }`: `active` from the `/rc
     active` (or legacy `Remote Control active`) indicator; `url` is a
     best-effort scan for a `claude.ai/...` link in plain text.
   - tmux `capture-pane -e` does not preserve OSC 8 hyperlink targets,
     so the footer link URL is usually *not* recoverable. When the
     session is active but no URL is in the grid, the copy/open commands
     toast a hint to use the footer link rather than failing silently.
   - Leader commands in view mode: `c` copy URL, `O` open in browser
     (via new `util::open_in_browser`, xdg-open/open).
   - In-view top-right badge `[remote ●]` (green) when active, combined
     with the existing direct-input badge; project-list rows show a
     cheap flag-based `[remote]` marker for RC-enabled features.
3. **Phase 3 — runtime control** — ✅ **toggle + config default done**;
   QR overlay deferred. Details:
   - Leader command `C` (not `R`, which is already "Refresh pane
     sizing") → `App::toggle_remote_control_in_view()` sends `C-u` then
     literal `/rc` then `Enter` straight to the focused Claude pane
     (bypassing the AMF composer, mirroring the compose slash-command
     path). Gated on a Claude session + `remote_control_block_reason`,
     toasting the reason when blocked.
   - Global default `AppConfig.remote_control_default` (serde default
     false). Applied in `start_create_feature` as
     `remote_control = default && remote_control_available`, so it never
     forces RC onto an incompatible session.
   - **QR overlay deferred**: it needs a new QR-encoding dependency and a
     render mode, and would encode the session URL — which tmux
     `capture-pane` usually can't recover (OSC 8 limitation, see Phase
     2). Low payoff until the URL is reliably obtainable, so skipped for
     now in favour of copy/open + the phone's own session list.
4. **Phase 4 (optional) — server mode**: a "remote server" session
   kind backed by `claude remote-control --spawn worktree` for
   multi-session-from-one-process workflows.

## 7. Open questions / decisions

- `cli_flags` signature: introduce `LaunchOpts` struct (recommended)
  vs. append at call site?
- Default session name: feature name, branch, or
  `amf-<branch>` prefix? (Affects the claude.ai session list.)
- Should RC default on globally, or strictly opt-in per feature /
  preset? (Recommend opt-in first, add global default in Phase 3.)
- QR rendering: worth it in-TUI, or just copy URL + rely on phone
  session list?

## 8. Testing

- Unit: `LaunchOpts` → flag vector (RC on/off, with/without name,
  Chrome combinations); preset (de)serialization with the new field;
  feature serde default round-trip.
- Unit: URL/indicator parser against captured-pane fixtures for
  v2.1.172 (`/rc active`) and older (`Remote Control active`).
- Mock `TmuxManager` (existing `expect_launch_claude`) to assert
  `--remote-control` is/ isn't passed per flag and per provider guard.
- Manual: real claude.ai login, create an RC feature, confirm the
  session appears at claude.ai/code and messages sync into the AMF
  pane.

## References

- Remote Control docs:
  <https://code.claude.com/docs/en/remote-control>
- Claude Code on the web (contrast):
  <https://code.claude.com/docs/en/claude-code-on-the-web>
