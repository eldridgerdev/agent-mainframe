# Per-session agent usage

- **Status:** Partial
- **Owner:** unassigned
- **Relates to:** token tracking (`src/token_tracking.rs`), session status
  sync (`src/app/sync.rs`), feature/session model (`src/project.rs`),
  dashboard list (`src/ui/list.rs`), agent sidebar (`src/ui/dashboard.rs`),
  launcher environment (`src/tmux.rs`), Claude/Codex/opencode hooks and
  plugins (`scripts/`, `.opencode/plugins/`)

## Why / problem

AMF should show two different usage scopes:

- Feature total usage on the dashboard feature row, next to the feature
  name.
- Per-agent-session usage next to the agent harness on the dashboard and
  inside the view sidebar.

The data model already points in the right direction: each
`FeatureSession` stores a `token_usage_source`, source-match confidence,
and transient `status_text`. The sync path also builds one usage job per
session. The problem is that source discovery falls back to workdir +
timestamp inference when a session does not have an exact harness session
ID. With multiple same-harness windows in one feature, more than one AMF
session can resolve to the newest provider transcript, making the visible
usage effectively feature-scoped or ambiguous.

## Current implementation

`FeatureSession` contains:

- `token_usage_source`: provider + provider session id.
- `token_usage_source_match`: `Exact` or `Inferred`.
- `status_text`: formatted, transient usage/status text shown in the UI.

`SessionTokenTracker` can read per-session usage for:

- Claude JSONL transcripts in `~/.claude/projects/.../{session_id}.jsonl`,
  including subagent JSONL files.
- Codex rollout JSONL files, preferably through Codex's
  `~/.codex/state_5.sqlite` thread index.
- opencode storage files under `storage/message/{session_id}` and
  `storage/part/{message_id}`.

Dashboard session rows already display `session.status_text`. The sidebar
builders already read usage from the selected `FeatureSession`. Exact
provider-session binding now exists for newly launched Claude, Codex, and
opencode sessions, with safer inference for older sessions. The main missing
part is feature-level aggregation for the feature row.

## Harness feasibility

### Claude

Feasible. Claude hooks expose a Claude `session_id`, and AMF already
parses Claude transcript usage by that id. For newly launched sessions,
AMF currently exports `AMF_SESSION` as the feature tmux session name; it
does not export the AMF `FeatureSession.id`, so hook events cannot reliably
bind the Claude session id back to the specific dashboard session row.

### Codex

Feasible. Codex rollout events include token counts, and AMF can resolve
exact rollout paths from the Codex state database when it knows the Codex
thread id. Current notify handling prefers `AMF_SESSION`, which identifies
the feature tmux session. Exact per-window usage needs the real Codex
session id preserved and mapped to the AMF `FeatureSession.id`.

### opencode

Feasible. opencode plugins see `sessionID`, and AMF already reads
step-finish token records by that id. New opencode windows need a way for
plugin events to tell AMF which `FeatureSession` they belong to.

### Pi

Not currently feasible from AMF's existing integration. Pi is launched as
an agent harness, but there is no `TokenUsageProvider::Pi`, no known local
usage artifact reader, and no hook/plugin integration for token usage.
Treat Pi as unsupported for this project until its CLI exposes usable
session usage metadata.

## Proposed design

Bind provider session IDs to AMF session IDs as early and exactly as each
harness allows. Use inference only for old sessions and recovery. Store
formatted per-session usage on each `FeatureSession` as today, but also
carry raw usage data long enough to compute a feature aggregate without
parsing display strings.

The UI rules should be:

- Feature row: compact aggregate usage for all agent harness sessions in
  the feature.
- Session row: usage for that exact agent session only.
- Sidebar: usage for the selected agent session only.
- Non-agent sessions: no token usage, except existing custom
  `status_text` behavior.
- Pi sessions: show no usage until a provider integration exists.

## Agile chunks

### P0 - Exact binding for launched sessions

Goal: stop newly launched sessions from relying on workdir/newest
inference when provider events can tell us the real session id.

- [x] Add launcher environment for agent windows:
  `AMF_FEATURE_SESSION_ID`, `AMF_TMUX_SESSION`, and `AMF_TMUX_WINDOW`.
- [x] Keep `AMF_SESSION` for backward compatibility while moving new code
  to the more explicit variables.
- [x] Update Claude hook scripts to include both the AMF feature-session id
  and Claude hook `session_id` in IPC payloads.
- [x] Update Codex notify handling to preserve the real Codex session id
  separately from the AMF tmux session id.
- [x] Update opencode plugins to include `AMF_FEATURE_SESSION_ID` when
  emitting sidebar/input/session events.
- [x] Add IPC handling that maps
  `AMF_FEATURE_SESSION_ID -> TokenUsageSource` and marks the source
  `Exact`.
- [x] Add regression tests for exact binding for Claude, Codex, and
  opencode.

Acceptance criteria:

- A new Claude/Codex/opencode session gets an exact source after the first
  useful provider event.
- Two same-harness windows in the same feature do not bind to the same
  source unless they are explicitly resumed to the same provider session.
- Existing resumed-session flows still set exact sources immediately.

### P1 - Safer inference fallback

Goal: preserve compatibility while making inferred usage visibly and
behaviorally secondary.

- [x] Keep current workdir/timestamp discovery for sessions with no exact
  source.
- [x] Prevent duplicate inferred sources among same-provider sessions in
  the same feature when there are multiple plausible provider sessions.
- [x] Prefer unmatched provider sessions closest to each
  `FeatureSession.created_at`.
- [ ] Keep `TokenUsageSourceMatch::Inferred` visible for Codex and extend
  confidence text to other providers if useful.
- [x] Replace an inferred source with an exact source as soon as an exact
  event arrives.
- [x] Add tests for multiple same-harness sessions created close together.

Acceptance criteria:

- Old sessions can still show usage.
- Ambiguous inference fails soft or stays marked inferred instead of
  confidently showing another session's usage.
- Exact events always win over inferred matches.

### P2 - Feature-level aggregate usage

Goal: show feature usage only on the feature row.

- [ ] Extend sync results or app state with raw `SessionTokenUsage` per
  `FeatureSession`, not only formatted `status_text`.
- [ ] Add an aggregation helper that sums agent-harness sessions per
  feature.
- [ ] Format the aggregate compactly for the dashboard feature row.
- [ ] Render the aggregate next to the feature name in `src/ui/list.rs`,
  before mode/review/plan badges.
- [ ] Exclude Terminal, Nvim, VSCode, Custom, and unsupported Pi sessions.
- [ ] Add UI tests/snapshots for feature rows with zero, one, and multiple
  agent usage sources.

Acceptance criteria:

- Feature rows show total agent usage for the feature.
- Session rows remain per-session.
- No aggregate is shown when no supported agent session has usage.

### P3 - Sidebar and dashboard placement cleanup

Goal: make the scope obvious in every visible location.

- [ ] Confirm sidebar builders always select the session matching
  `ViewState.window` before falling back to same-kind sessions.
- [ ] Keep sidebar usage derived from the selected `FeatureSession` only.
- [ ] Decide whether session-row usage stays on its current second line or
  moves beside the harness label.
- [ ] If moved inline, add truncation/width handling so long cost strings
  do not crowd labels.
- [ ] Add tests for switching between two agent windows with different
  usage values.

Acceptance criteria:

- Viewing Claude 1, Claude 2, Codex 1, or opencode 1 shows only that
  session's usage in the sidebar.
- Dashboard session display clearly reads as per-session usage.
- Feature aggregate remains visible only on feature rows.

### P4 - Provider-specific polish and observability

Goal: make the integration debuggable and robust across provider versions.

- [ ] Log source binding changes with provider, match confidence, AMF
  session id, and provider session id.
- [ ] Add debug log entries when inference is skipped because a match is
  ambiguous.
- [ ] Add a small internal helper to inspect a selected session's usage
  source and confidence.
- [ ] Document Pi as unsupported for usage until a concrete source is
  identified.
- [ ] Add changelog/release-note language once implementation starts.

Acceptance criteria:

- A wrong or missing usage line can be diagnosed from AMF debug logs.
- Unsupported providers fail quietly in the UI but are explicit in docs.

## Suggested release slices

### Slice 1: correctness foundation

Ship P0 + enough P1 to prevent duplicate inferred matches. This fixes the
core correctness issue without changing the feature row.

### Slice 2: requested UI behavior

Ship P2 + P3. This delivers feature totals on feature rows and
session-scoped usage in the dashboard/sidebar.

### Slice 3: hardening

Ship P4 plus any provider-specific follow-ups found during dogfooding.

## Open questions

- Should feature aggregates include inferred sources, or only exact
  sources? A conservative default is exact-only, with inferred included
  only when there is no ambiguity.
- Should the feature aggregate show full `in/out/eff/cost`, or a shorter
  `usage <eff> · <cost>` form to protect row width?
- Should session rows keep their current two-line layout or move usage
  beside the harness label?
- Can Pi expose a local transcript, API, hook, or CLI command with
  per-session usage?

## Reasoning / when to build

Build this before adding more session-sidebar features or more agent
harnesses. The underlying data model is already close; the highest-value
work is binding provider session IDs exactly and making the UI scopes
honest. Doing this now also reduces future complexity because new harnesses
can be required to implement explicit session binding from the start.
