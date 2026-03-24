# Opencode Sidebar Plan

## Findings

There is already a sidebar implementation in the repo, but it is currently
Claude-only:

- [`src/app/state.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/app/state.rs:115)
  only enables the sidebar for `SessionKind::Claude`
- [`src/ui/dashboard.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/ui/dashboard.rs:11)
  builds Claude-specific sidebar data
- [`src/ui/pane.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/ui/pane.rs:30)
  renders a Claude-branded sidebar shell

Like Codex, the existing sidebar shell can be generalized. Unlike Codex,
Opencode already exposes much richer structured surfaces that AMF can use
without redesigning the whole runtime:

- official plugin hooks
- local session/message/token storage
- JSON session listing / export flows
- an optional HTTP server API

That makes Opencode the best candidate for a richer sidebar on the current
tmux-based architecture.

## Opencode Data We Can Get Today

### Stable and already integrated

These are already available in the current AMF architecture:

- **Session / feature identity** from project state
- **Branch / mode / review / plan-mode flags** from project state
- **Waiting-for-input** from the existing Opencode input-request plugin in
  [`.opencode/plugins/input-request.js`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/.opencode/plugins/input-request.js:1)
- **Diff-review / change-reason metadata** from the existing Opencode
  change-tracker plugin in
  [`.opencode/plugins/change-tracker.js`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/.opencode/plugins/change-tracker.js:1)
- **Session restore choices** from `opencode session list --format json` in
  [`src/app/opencode.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/app/opencode.rs:328)
- **Token usage, including reasoning tokens** from local Opencode storage in
  [`src/token_tracking.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/token_tracking.rs:352)

### Available from local Opencode storage

By inspecting `~/.local/share/opencode/storage`, Opencode already persists:

- session metadata with `id`, `title`, `directory`, `updated`, and code-change
  summary fields such as additions / deletions / files
- user message records keyed by `sessionID`
- message parts containing user prompt text
- token accounting in per-part `step-finish` records

That means AMF can add local parsers for:

- latest prompt
- latest session title
- code-change summary (`+/-/files`)
- per-session token / reasoning usage

This is a significantly stronger local data source than the current Codex
transcript path.

### Available from supported Opencode plugin hooks

The official Opencode plugin system exposes events such as:

- `session.status`
- `session.diff`
- `todo.updated`
- `permission.asked` / `permission.replied`
- `message.updated`
- `tool.execute.before` / `tool.execute.after`
- `lsp.updated`
- `file.edited`

AMF is already using that surface today for input requests and diff review.
That means a richer Opencode sidebar can be built by extending the existing
local worktree plugins to write structured sidecar state for AMF.

### Available from supported Opencode server integration

Opencode also exposes an HTTP server via `opencode serve`. That is useful, but
it should be treated as optional later work, not the first path. The plugin and
storage surfaces are already enough for a strong sidebar plan.

## What Opencode Does Not Cleanly Support

These are the main limits to call out.

1. **AMF does not currently parse Opencode prompt history or session summaries
   for the sidebar.**
   The data exists locally, but there is no Rust parser for it yet.

2. **The embedded tmux pane is not a reliable source of structured Opencode
   state.**
   Opencode has its own TUI sidebar / status concepts, but AMF should not try
   to scrape them from ANSI output. Use storage, plugin sidecar files, or the
   server API instead.

3. **Fresh-session per-window correlation still needs explicit tracking.**
   When AMF restores an Opencode session, it knows the exact session id. For
   newly launched sessions, AMF still relies on discovery by workdir and
   timestamps until a concrete session id is learned. Plugin sidecar data should
   therefore be keyed by `sessionID` whenever possible.

4. **Some rich state is event-driven, not query-driven, on the current plugin
   path.**
   For things like todos, permissions, or live diffs, the plugin path works
   best when AMF persists a rolling sidecar file. Without that sidecar, the
   dashboard has no current source of truth to render.

5. **An HTTP server integration would add lifecycle complexity.**
   `opencode serve` is promising, but it introduces another long-lived process,
   endpoint management, and reconnect behavior. That is why it should stay
   optional unless the plugin/storage path proves insufficient.

## Recommended Plan

### Phase 1. Ship an Opencode sidebar on the existing tmux architecture

Goal:

- get immediate parity with the current Claude sidebar
- reuse the generalized sidebar shell that Codex also needs
- use data AMF already has plus low-risk local Opencode storage reads

Files:

- [`src/app/state.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/app/state.rs:115)
- [`src/ui/dashboard.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/ui/dashboard.rs:11)
- [`src/ui/pane.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/ui/pane.rs:30)
- [`src/main.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/main.rs)
- new Opencode storage helpers under `src/app/` or `src/token_tracking.rs`

Implementation:

1. Generalize the sidebar gate so `SessionKind::Opencode` can use it too.

2. Reuse the same four base sections:
   `Session`, `Status`, `Prompt`, `Summary`

3. Populate `Session` with:
   - project / feature / branch
   - session label
   - Opencode session title when known
   - vibe/review/plan mode

4. Populate `Status` with:
   - activity (`Thinking`, `Ready`, `Waiting for input`)
   - token usage, including reasoning tokens
   - code-change summary from session storage when available

5. Populate `Prompt` from Opencode local storage by reading the latest user
   message / message part for the active session.

6. Populate `Summary` from the existing AMF feature summary field first.

This phase should ship before any richer Opencode-specific work.

### Phase 2. Add a rich Opencode sidebar via local plugin sidecar state

Goal:

- expose the best structured Opencode state without changing the runtime model
- use officially supported plugin hooks instead of ANSI scraping

Files:

- [`.opencode/plugins/input-request.js`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/.opencode/plugins/input-request.js:1)
- [`.opencode/plugins/change-tracker.js`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/.opencode/plugins/change-tracker.js:1)
- new `.opencode/plugins/sidebar-state.js`
- [`src/app/setup.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/app/setup.rs:516)
- new Rust reader for `.amf/opencode-sidebar/<session-id>.json`
- [`src/ui/dashboard.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/ui/dashboard.rs:11)

Implementation:

1. Add a dedicated Opencode plugin that listens for:
   - `session.status`
   - `session.diff`
   - `todo.updated`
   - `permission.asked`
   - `permission.replied`
   - `message.updated`
   - `tool.execute.before`
   - `tool.execute.after`
   - optionally `lsp.updated`

2. Have the plugin write a small structured sidecar JSON file under the
   feature workdir, for example:
   `.amf/opencode-sidebar/<session-id>.json`

3. Store fields such as:
   - session id
   - current status
   - last active tool
   - latest prompt preview
   - todo list or todo count
   - diff summary
   - pending permission request
   - last error
   - optional LSP health summary

4. Extend the Opencode sidebar sections to use that sidecar:
   - `Session`
   - `Status`
   - `Work`
   - `Todos`
   - `Summary`

5. Keep AMF resilient:
   if the plugin sidecar is absent or stale, fall back to the simpler Phase 1
   storage-based sidebar.

This is the main path to "rich Opencode parity" and should be preferred over an
immediate server integration.

### Optional Later Project. Direct Opencode server integration

Goal:

- use `opencode serve` as a first-class structured backend if the plugin path
  proves too limited or too fragile

This is intentionally separate from the core Opencode sidebar plan.

Recommended design:

1. Add an Opencode runtime abstraction for optional server-backed sessions.

2. Start or attach to `opencode serve` and consume structured session APIs /
   event streams instead of relying on sidecar files.

3. Treat the server as the source of truth for:
   - current session status
   - todos
   - diffs
   - messages
   - permissions
   - tooling / service health

4. Keep the plugin/storage path as a fallback so AMF still works when the
   server is unavailable.

This should only be started if the Phase 2 plugin-sidecar approach fails to
cover the needed UX.

## Delivery Order

1. **Phase 1 first.**
   The general sidebar shell should support Opencode as soon as it supports
   Codex.

2. **Add local Opencode prompt/session parsers immediately after Phase 1 if
   needed.**
   Those are low-risk and use existing storage.

3. **Build Phase 2 before any server work.**
   The official plugin events already expose most of the structured state AMF
   would want in a sidebar.

4. **Only then decide whether to start the optional server project.**
   Do not pay server-process complexity unless the plugin/storage design proves
   insufficient.

## Testing Plan

### Phase 1 tests

- update layout tests in
  [`src/ui/pane.rs`](/home/eldridger/code/claude_super_vibeless/.worktrees/codex-sidebar/src/ui/pane.rs)
  so sidebar width math works for Opencode too
- add a render test for `Opencode view shows sidebar`
- add parser tests for Opencode latest-prompt extraction from storage
- add parser tests for Opencode session title / additions / deletions / files

### Phase 2 tests

- fixture-driven tests for the new Opencode sidebar sidecar reader
- plugin payload fixture tests for:
  - `session.status`
  - `todo.updated`
  - `session.diff`
  - `permission.asked`
  - `message.updated`
- manual validation that sidecar state updates while Opencode runs inside tmux
- manual validation that AMF degrades cleanly when plugin files are missing

### Manual checks

- fresh Opencode feature
- restored Opencode session via `S`
- Opencode feature in vibeless diff-review mode
- Opencode feature waiting for input
- Opencode feature with active todos
- narrow terminal width fallback
- leader menu / help / prompt dialog overlays while sidebar is visible
- multiple Opencode sessions in one feature

### Optional server project tests

- reconnection behavior when the HTTP server restarts
- fallback to plugin/storage path when the server is unavailable
- event ordering / stale state handling across reconnects

## Recommendation

The repo should treat this as **two practical projects plus one optional one**:

1. **Opencode sidebar parity in the current viewer**
   This should ship alongside the generalized sidebar work.

2. **Rich Opencode sidebar via plugin sidecar state**
   This is the preferred richer path because it builds on the documented plugin
   system AMF already uses.

3. **Optional direct server integration**
   This should remain separate unless the plugin/storage path turns out to be
   inadequate.

If the question is "can we build an Opencode sidebar now?", the answer is
**yes**.
If the question is "can we get rich Opencode-specific state without a server
rewrite?", the answer is also **yes, probably**, because the plugin system is
already strong enough to support it.
