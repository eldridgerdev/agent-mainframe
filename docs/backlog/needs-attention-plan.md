# Needs Attention: why a stopped agent stopped

- **Status:** In progress — the feature is built and shipping. What
  remains is the live four-harness walkthrough (no automated check
  substitutes for real Claude / Codex / OpenCode / Pi sessions on a
  running dashboard) and the two deferred fidelity gaps below: **Pi
  reports nothing at all**, and Codex cannot distinguish a question from
  a completion.
- **Owner:** unassigned
- **Relates to:** thinking-status sync (`src/app/sync.rs` —
  `sync_thinking_status`), notification hooks (`src/app/setup.rs`,
  `scripts/attention.sh`, `scripts/codex-notify.sh`,
  `.opencode/plugins/input-request.js`), IPC ingestion
  (`src/app/notifications.rs`), dormancy (`src/app/dormant.rs`), the
  agent gate (`src/resources/limits.rs`,
  `src/app/resource_gate.rs`)

## Why / problem

A stopped agent has stopped for a reason, and "waiting for input" does
not say which. The dashboard already knew *that* a session went idle —
`sync_thinking_status` watches the thinking transition and raises a
generic pending input — but a user scanning several running features
still had to open each one to find out whether it was blocked on a
question or had quietly finished. That is the one thing the dashboard
was not telling them.

## Proposed design

An in-memory **attention layer** over the persisted status, fed by the
harnesses' own lifecycle hooks:

- `AttentionState` is `Question`, `CompletedAwaitingReview`, or
  `Waiting`, in that deliberate sort order — a question always outranks
  a completion on the same feature.
- `HarnessCapabilities::for_agent` declares once, per harness, what it
  can justify claiming; `resolve` narrows any state the harness cannot
  prove down to `Waiting`, so the UI never shows a distinction the
  signal doesn't support.
- Ingestion is a **separate** `type: "attention"` IPC message carrying
  `amf_event_kind`, not an extension of the notification payload, so new
  harness wiring cannot disturb the existing pending-input flow.
- Nothing is persisted: the map lives on `App`, is rebuilt from events,
  and is empty after a restart. No `amf.db` migration, and
  `ProjectStatus` is untouched — the dashboard composes the two at
  render time.
- Clearing rides the **existing** thinking transition rather than adding
  a second mechanism; harnesses with no new-output signal fall back to
  clearing when the user opens the session.
- `waiting_stale_minutes` (default 30, `0` disables) ages a record out
  into plain idle. Ageing **drops** the record rather than storing an
  `Idle/stale` variant — an absent record already renders as ordinary
  idle.

The layer is advisory throughout: it explains a stop, it never excuses
one. A waiting session still counts toward `max_concurrent_agents` and
still qualifies as dormant.

## Progress

- [x] `AttentionState`, `AttentionRecord`, and the in-memory map on
      `App` (`src/app/attention.rs`); no persistence, no migration.
- [x] Per-harness `HarnessCapabilities` + `resolve` / `clears_on_open`.
- [x] `waiting_stale_minutes` config key, threshold accessor, and
      ageing (`age_out_attention`). **Global-only**, matching its
      siblings: `.amf/config.json` merging covers `ExtensionConfig`
      only, so `max_concurrent_agents`, `low_memory_warn_mb`, and
      `dormant_*` have no project override either.
- [x] `type: "attention"` IPC message + emitters: `scripts/attention.sh`
      (argv[1] = kind) wired to Claude `Stop`→`completed` and
      `Notification`→`question`; `codex-notify.sh`→`completed`;
      opencode `input-request.js`→`question` / `completed` / `clear`.
- [x] Ingestion in `App::handle_ipc_message`; unknown/absent kinds
      degrade to `Waiting`.
- [x] Clearing on the thinking transition, on feature stop, and on
      `enter_view` for `clears_on_open()` harnesses. A standing
      `Question` is never downgraded to a `completed`.
- [x] `needs_attention()` / `attention_counts()` / `feature_attention()`
      / `attention_rows()` — the shared ordered query behind every
      surface.
- [x] Feature rows (`src/ui/list.rs`): per-state glyph and colour, with
      ASCII fallbacks when `nerd_font` is off.
- [x] Header count broken out by kind (`src/ui/header.rs`), with
      `badge_text()` shared with mouse hit-testing and a visible `<leader i>`
      hint for opening the needs-attention list.
- [x] The `i` overlay rebuilt as one needs-attention list; a feature's
      generic wait folds into its attention row, while diff reviews /
      change reasons / review-ready keep rows of their own. "Generic
      wait" is `PendingInput::is_session_wait()` — `input-request` *and*
      the bare `stop` that Claude's Stop hook leaves when it forwards
      its own payload, which had been folding, clearing, and ageing
      differently from its own synonym.
- [x] **One pending wait per session** (`App::queue_pending_input`). A
      stop is a standing fact about a session, not an item of work, and
      every harness re-reports it — Claude's Stop hook at every turn
      boundary. Each report used to append a row, so one waiting session
      could put hundreds of identical entries in `i` and the header
      count. A re-report now replaces the entry (newest message wins)
      and does not toast again; the file scan collapses the same way, so
      a stop on disk in both the feature and global directories is one
      row. Discrete work is untouched: two diff reviews are two
      requests.
- [x] Embedded session view: sidebar status line reports the state, and
      leader `i` opens the same list.
- [x] Help overlay and leader-menu wording.
- [x] Regression guard that a waiting session still trips the agent gate
      and still qualifies as dormant.
- [x] `README.md` ("See which agents need you") and `CHANGELOG.md`.
- [ ] Live four-harness walkthrough on a running dashboard: a question,
      a completion, and a subsequent new-output event per harness.
- [ ] **Pi support.** Pi has no hook mechanism (`src/pi.rs` is a version
      probe) and nothing writes its thinking file, so Pi gets no signal
      at all — not even the generic `Waiting` the design intended for a
      degraded harness. Its `HarnessCapabilities` are all-false and its
      rows carry no state.
- [ ] **Codex question fidelity.** `codex-notify.sh` fires once at turn
      end with a fixed payload, so a Codex question shows as `Waiting`.

## Open questions

- **Is a tmux `window_activity` fallback the right answer for Pi?** It
  is the signal `src/app/dormant.rs` already reads, so the plumbing
  exists, but it reports *silence*, not *blocked* — a Pi session
  thinking quietly for a minute would look the same as one waiting for
  an answer. A wrong `Waiting` may be worse than no state.
- **Ageing vs. dormancy.** With ageing at 30 minutes and
  `dormant_idle_minutes` at 60, a completed session ages out of the
  needs-attention list and can then surface under `z`. Plausibly
  correct — it is genuinely unattended — but it was never explicitly
  designed, and the two thresholds have no relationship enforced
  between them.
- **Feature-row roll-up precedence** (a question outranking a
  completion) is an implementation choice, not a stated decision.
  Attention is keyed per tmux session and a feature owns exactly one, so
  nothing exercises the rule today; it will start to matter if
  attention ever becomes per-session.
- **Restart loses real signal.** Sessions genuinely waiting before a
  restart read as ordinary active until their next event. This follows
  directly from the decision not to persist, and is an accepted cost
  rather than a defect — but it is the most likely thing to be
  revisited if it annoys in practice.

## Reasoning / when to build

Built. The two unchecked fidelity items are worth picking up when
someone actually runs Pi or Codex often enough to be bothered — both are
additive (a capability flag flips, one emitter changes) and neither
disturbs the harnesses already at full fidelity, because
`HarnessCapabilities` is the single place the difference lives.
