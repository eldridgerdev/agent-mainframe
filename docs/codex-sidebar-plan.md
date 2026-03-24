# Codex Sidebar Plan

## Findings

There is already a sidebar implementation in the repo, but it is hard-gated to
Claude sessions today:

- [`src/app/state.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/app/state.rs:115)
  only enables the sidebar when `session_kind == SessionKind::Claude`
- [`src/ui/dashboard.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/ui/dashboard.rs:11)
  builds `ClaudeSidebarData`
- [`src/ui/pane.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/ui/pane.rs:30)
  renders a Claude-branded sidebar shell

The good news is that most of the current sidebar content is already AMF-native,
not Claude-specific:

- feature identity, branch, vibe/review mode
- pending-input count
- thinking state
- per-session `status_text`
- latest prompt via `latest_prompt_for_session()`
- feature summary

That means a first Codex sidebar does not require a new backend. AMF can reuse
almost all of the current sidebar plumbing if the Claude-specific names and
guards are generalized.

## Codex Data We Can Get Today

### Stable and already integrated

These are already available in the current AMF architecture:

- **Session / feature identity** from project state
- **Branch / mode / review / plan-mode flags** from project state
- **Waiting-for-input** from the existing Codex notify hook in
  [`scripts/codex-notify.sh`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/scripts/codex-notify.sh:1)
- **Thinking state** from existing prompt-submit + thinking-stop flow in
  [`src/app/sync.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/app/sync.rs:298)
- **Latest prompt** from `.claude/latest-prompt.txt`,
  `.codex/latest-prompt.txt`, or the Codex session store via
  [`src/app/util.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/app/util.rs:48)
- **Token usage / status line** from the existing Codex token tracker in
  [`src/token_tracking.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/token_tracking.rs:255)
- **Saved session title / restore choices** from the existing Codex session
  parser in
  [`src/app/codex_sessions.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/app/codex_sessions.rs:1)

### Available from local Codex session JSONL, but not a stable contract

By inspecting local `~/.codex/sessions/...jsonl` files and AMF's existing
parsers, Codex session logs currently contain:

- `event_msg:user_message`
- `event_msg:agent_message`
- `event_msg:token_count`
- `response_item:message`
- `response_item:reasoning`
- `response_item:function_call`
- `response_item:function_call_output`
- `response_item:custom_tool_call`
- `response_item:custom_tool_call_output`

That means AMF can reverse-engineer extra sidebar details such as:

- last tool name
- last command arguments
- recent agent commentary text

But this is a **best-effort parser**, not a supported Codex integration surface.
AMF should treat transcript-derived enrichments as optional and degrade
gracefully when fields move.

### Available from supported Codex app-server integration

Local investigation plus the official Codex docs show that `codex app-server`
provides structured live events for:

- plan updates
- reasoning summary deltas
- command execution start / output / completion
- file change proposals and approvals
- request-user-input events
- thread list / thread read / thread resume style flows

This is the only supported path I found for a **rich live Codex sidebar**.
If we want more than AMF-native metadata, app-server is the right integration
point, not more transcript scraping.

## What Codex Does Not Cleanly Support

These are the important limits to call out in the plan.

1. **There is no supported live event stream for the existing tmux-launched
   interactive Codex CLI session.**
   If AMF keeps launching plain `codex` inside tmux, richer live data has to
   come from notify hooks or transcript parsing. For supported structured live
   events, AMF would need to integrate `codex app-server` instead of treating
   the CLI as a black-box terminal process.

2. **Codex session JSONL is not a stable public contract.**
   AMF already parses it for restore/title/latest-prompt/token usage, but any
   new sidebar sections built from raw JSONL beyond that should be considered
   opportunistic, not guaranteed across Codex CLI upgrades.

3. **Reasoning summaries are not available from the current local JSONL in a
   reliable plain-text form.**
   In local samples, `response_item:reasoning` had empty `summary` plus
   encrypted content. So a "Reasoning" sidebar section is not realistic on the
   current tmux/CLI path. If we want readable reasoning summaries, we need the
   app-server event stream.

4. **There is no documented Codex equivalent of opencode-style Todo / Context /
   LSP state.**
   We can expose plan state, tool activity, and file changes, but not a real
   todo tree or IDE/LSP context unless Codex adds a supported surface for it.

5. **Per-window correlation is weak when multiple Codex sessions share the same
   workdir.**
   Current discovery in
   [`src/app/codex_sessions.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/app/codex_sessions.rs:19)
   and
   [`src/token_tracking.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/token_tracking.rs:210)
   is primarily keyed by workdir and timestamps unless a specific restored
   session id is already known. That is good enough for feature-level status,
   but not a perfect source for a per-window "current Codex thread" sidebar.

## Recommended Plan

### Phase 1. Ship a Codex sidebar on the existing tmux architecture

Goal:

- get immediate parity with the current Claude sidebar
- use only data AMF already owns or already parses
- avoid coupling the first delivery to a Codex runtime redesign

Files:

- [`src/app/state.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/app/state.rs:115)
- [`src/ui/dashboard.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/ui/dashboard.rs:11)
- [`src/ui/pane.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/ui/pane.rs:30)
- [`src/main.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/main.rs)

Implementation:

1. Replace the Claude-only sidebar gate with an agent-aware gate.
   Recommended shape:
   `ViewState::sidebar_kind() -> Option<SessionKind>`
   with `Claude` and `Codex` enabled, everything else disabled.

2. Rename sidebar data types to remove Claude-specific naming.
   Example:
   `ClaudeSidebarData` -> `AgentSidebarData`

3. Render the same four sections for Codex that Claude already uses:
   `Session`, `Status`, `Prompt`, `Summary`

4. Brand the title and accent colors by agent:
   `Claude Sidebar` for Claude
   `Codex Sidebar` for Codex

5. Reuse existing generic data:
   - target project / feature / branch
   - session label
   - vibe/review/plan mode
   - activity (`Thinking`, `Ready`, `Waiting for input`)
   - status/token text
   - latest prompt
   - feature summary

This phase should be treated as the minimum shippable implementation.

### Phase 2. Add low-risk Codex-specific polish without changing runtimes

Goal:

- improve the sidebar for Codex without committing to app-server yet

Files:

- [`src/app/codex_sessions.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/app/codex_sessions.rs:1)
- [`src/app/util.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/app/util.rs:48)
- [`src/ui/dashboard.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/ui/dashboard.rs:11)

Implementation:

1. Show the restored Codex thread title when a concrete session id is known.

2. Prefer Codex session-store prompt history when available, not just the hook
   cache file.

3. Add a Codex-only status line field for:
   - session id or shortened thread id
   - token usage provider match confidence

4. Keep this phase conservative:
   do not build a live "Reasoning" section from transcript parsing
   do not promise exact per-window tool state

This phase is only worth doing if Phase 1 feels too bare after real use.

### Optional Later Project. Rich Codex sidebar via `codex app-server`

Goal:

- reach the closest thing to real Codex parity
- stop depending on reverse-engineered JSONL for live behavior

This is intentionally **not** part of the core Codex sidebar plan. It is a
separate architecture project we can come back to later if the simpler tmux
sidebar is not enough.

Recommended design:

1. Add a Codex runtime abstraction alongside the existing tmux launch path.

2. For Codex features, launch a small AMF-managed Codex client that talks to
   `codex app-server` and stores structured thread state in memory.

3. Keep tmux only for the visible terminal if needed, but treat app-server as
   the source of truth for sidebar state.

4. Persist a Codex thread model in AMF with fields such as:
   - thread id
   - current turn id
   - current plan
   - current reasoning summary
   - current command execution item
   - current file-change item
   - pending approval / input request
   - live token usage

5. Extend the sidebar sections for Codex only:
   - `Session`: target, thread id, mode, branch
   - `Status`: activity, approvals pending, token usage
   - `Plan`: current plan text or latest plan update
   - `Work`: current command / file change / last tool
   - `Summary`: feature summary or latest agent summary text

6. Wire AMF notifications and diff review to app-server requests where possible,
   instead of relying on file-based hook handshakes.

This optional project is the only path that cleanly unlocks readable reasoning
summaries, plan streaming, command output streaming, and file-change approval
state.

## Delivery Order

1. **Phase 1 first.**
   It is cheap, useful, and already justified by the current sidebar structure.

2. **Review the result in real Codex sessions.**
   Decide whether the generic sidebar is already "good enough".

3. **Skip Phase 2 unless users still want more context.**
   Transcript enrichment has limited upside and real maintenance cost.

4. **Only then decide whether to start the optional app-server project.**
   If the goal is true live Codex telemetry, app-server is the right project.
   If the goal is just "Codex has the same sidebar shell as Claude", Phase 1 is
   enough.

## Testing Plan

### Phase 1 tests

- update layout tests in
  [`src/ui/pane.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/ui/pane.rs)
  so sidebar width math works for Codex too
- add a render test for `Codex view shows sidebar`
- keep the existing `non-Claude/non-Codex stays full-width` case
- update any resize expectations so Codex pane capture uses main-content width
  when the sidebar is present

### Manual checks

- fresh Codex feature
- restored Codex session via `S`
- Codex feature with waiting input notification
- Codex feature with summary present
- narrow terminal width fallback
- leader menu / help / prompt dialog overlays while sidebar is visible
- feature with multiple Codex sessions in one workdir

### Optional app-server project tests

- unit tests for app-server event reducers
- fixture-driven tests for `plan`, `reasoning`, `commandExecution`,
  `fileChange`, and `requestUserInput`
- reconnection / process-restart behavior
- downgrade path when `codex app-server` is unavailable

## Recommendation

The repo should treat this as **two different projects**:

1. **Codex sidebar parity in the current viewer**
   This is straightforward and should be built now.

2. **Optional rich live Codex telemetry**
   This is feasible, but only if AMF adopts `codex app-server` as a first-class
   integration instead of trying to stretch the current tmux + transcript model
   too far.

If the question is "can we build a Codex sidebar now?", the answer is **yes**.
If the question is "can we match every richer thing Codex knows about the run
without changing architecture?", the answer is **no**.
