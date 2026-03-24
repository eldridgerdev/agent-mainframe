# Claude Sidebar Plan

## Findings

The current viewing path is single-pane all the way through: `src/ui/dashboard.rs`
calls `src/ui/pane.rs`, and that renderer treats everything below the header
as one content area. There is no sidebar/layout abstraction yet.

The main constraint is in `src/main.rs`: while viewing, AMF resizes the tmux
window to the full terminal width and captures that full pane. If we add a
right sidebar, AMF must resize the Claude tmux pane to the left content width,
not the full width, or the rendered Claude output will be wrong.

AMF already has some reusable data:

- per-session status text / token usage in `src/app/sync.rs` and `src/project.rs`
- feature summaries in `src/project.rs` and `src/app/sync.rs`
- pending input state in `src/app/sync.rs`

What AMF does not have is an opencode-like structured Claude feed for things
like todos, context blocks, or LSP state. The UI shell is straightforward;
matching opencode's content is a separate instrumentation problem.

## Progress

- [x] Add a Claude-only sidebar layout and resize the tmux pane to the left
  content width
- [x] Ship a blank sidebar checkpoint for UI review before wiring in data
- [x] Add theme-based styling for the sidebar shell
- [x] Add prompt context with timestamp
- [x] Add live activity detail from thinking/tool hooks
- [x] Add waiting/input detail from pending notifications
- [x] Polish session / feature metadata
- [x] Investigate Claude hooks and local artifacts for richer sidebar data
- [x] Implement current-task support from Claude task events/transcripts
- [x] Render a real `Todos` list from Claude task state in the sidebar
- [x] Add transcript fallback when `claude_session_id` is missing
- [x] Improve todo styling and widen the sidebar for better readability
- [x] Add a focused todo viewer with a visible sidebar keybind hint
- [ ] Add a show/hide sidebar keybind, defaulting to shown
- [ ] Remove the `Session` section
- [ ] Remove the `Summary` section
- [ ] Auto-scroll the todo list so the active and next unfinished items stay in
  view
- [ ] Investigate whether `TodoWrite` can become a real source of truth
- [ ] Decide whether to add a richer current-task model beyond the task list

## Implementation Plan

### 1. Add a Claude-sidebar layout model

Files:

- `src/ui/pane.rs`
- `src/ui/dashboard.rs`
- `src/app/state.rs`

Goal:

- split the view into `main pane + sidebar` for Claude sessions only
- preserve current full-width behavior for opencode and codex

### 2. Implement the testing checkpoint with a blank sidebar

Files:

- `src/ui/pane.rs`
- `src/main.rs`

Goal:

- reserve the right-side width
- resize tmux to the left pane width
- render an empty/sidebar-shell panel with section headers only

This is the checkpoint where the UI can be judged before any content work
starts: spacing, borders, width, and whether Claude feels too cramped.

### 3. Validate the shell before adding data

Manual checks:

- normal Claude view
- leader menu overlay
- scroll mode
- transient bottom message
- narrow terminal widths
- dialogs opened from view mode

### 4. Fill the sidebar with AMF-native data first

Use existing state rather than Claude output parsing.

First pass:

- session / feature identity
- waiting for input
- latest summary
- token / status text
- maybe workdir, branch, and mode

### 5. Decide whether to add richer Claude instrumentation

If we want opencode-style `Todo / Context / current task` sections, AMF will
need Claude-specific structured sidecar data, probably via local files or
notification hooks under the worktree. That should be treated as a second
project after the blank-shell and basic metadata pass.

Before building that richer layer, add an explicit investigation step:

- inspect existing Claude hooks, notification files, latest-prompt storage,
  review hooks, and any other AMF-managed feature metadata
- determine which useful sidebar data already exists and can be surfaced
  cheaply
- identify which missing pieces would require new Claude-side hooks or local
  sidecar files
- compare those options against other existing AMF mechanisms so we reuse
  stable plumbing instead of inventing a parallel system unnecessarily

Investigation result so far:

- existing Claude hooks already emit enough data for a richer first pass:
  - `UserPromptSubmit` persists `.claude/latest-prompt.txt`
  - `thinking-start` / `thinking-stop` track active thinking
  - `tool-start` / `tool-stop` track active tool execution
  - notification files / IPC already carry input requests and diff-review events
- that means the next sidebar improvements should prefer existing AMF state and
  cached prompt metadata before adding any new Claude-specific sidecar format
- only higher-order concepts such as opencode-style todo lists, structured
  context blocks, or explicit current-task models appear to require new Claude
  instrumentation

### 6. Next sidebar metadata passes

Use existing AMF-managed metadata only. Do not add new Claude-specific
instrumentation for these steps.

Ordered list:

1. Prompt context
   - show only the latest prompt
   - include a timestamp for that latest prompt
   - keep the prompt section focused on concise context, not full history

2. Live activity detail
   - show current thinking state
   - show current tool execution state
   - show the active tool name when available from `tool-start` / `tool-stop`

3. Waiting / input detail
   - show what Claude is waiting on, not just a count
   - surface the pending input/request message when available
   - keep this focused on actionable waiting state

4. Exclusions for this pass
   - do not show diff-review status in the sidebar
   - do not add todo lists, current-task models, or other new structured
     Claude-side data yet

5. Session / feature metadata polish
   - improve ordering and wording of project / feature / session / mode /
     branch metadata
   - tune truncation and wrapping so the sidebar reads cleanly at current width
   - keep summary and token usage readable without overloading the layout

Status:

- prompt context: implemented
- live activity detail: implemented
- waiting / input detail: implemented
- session / feature metadata polish: implemented

### 7. Next UI follow-up pass

Queue these for the next working session.

1. Add sidebar expansion keybinds
   - add keybinds to open sidebar sections in a larger focused view
   - the todo list viewer and its sidebar hint are now implemented
   - decide whether any other sidebar section needs an expanded view
   - add a keybind to show/hide the sidebar entirely
   - keep the sidebar shown by default

2. Simplify sidebar sections
   - remove the `Session` section because that information is already visible in
     the top bar
   - remove the `Summary` section
   - rebalance the remaining vertical space toward `Todos` and `Prompt`

3. Improve todo list viewport behavior
   - ensure the todo list automatically scrolls so the current
     `in_progress` item is visible
   - ensure the next not-yet-done item is also visible when possible
   - keep the list centered on the most actionable items rather than the top of
     the raw task list

### 8. Later instrumentation work

These are explicitly in scope for the sidebar, but should be treated as a later
phase after the existing-metadata passes above.

1. Real todo list
   - investigate whether Claude hooks, local sidecar files, or another AMF
     mechanism can provide a durable structured todo list
   - prefer a source that is local to the worktree and stable across refreshes

2. Current task
   - investigate how to represent Claude's current task in a way that is more
     reliable than inferring it from a single prompt preview
   - prefer structured task/context metadata over brittle text scraping

Investigation result so far:

- AMF still does not have its own real structured todo artifact for Claude
  sessions, but Claude Code itself now appears to have two relevant internal
  stores under `~/.claude/`:
  - `tasks/<session_id>/<n>.json`
  - `todos/<session_or_agent_id>-agent-<session_or_agent_id>.json`
- `tasks/` is the strongest lead:
  - Claude session transcripts show real `TaskCreate` and `TaskUpdate` tool
    calls with structured inputs such as `subject`, `description`,
    `activeForm`, `taskId`, and `status`
  - Claude debug logs show hook matching on `TaskUpdate`, which means these
    task tools are hookable by name in the existing hook system
  - task directories appear to be keyed by Claude `sessionId`, which AMF
    already stores as `claude_session_id`
- `todos/` exists, but the current evidence is weaker:
  - the files are plain JSON arrays and many are empty
  - local transcripts did not yet surface a concrete `TodoWrite` event or a
    non-empty todo file
  - todo filenames look session/agent keyed, but the exact stability and
    schema need validation before AMF should depend on them directly
- existing AMF-managed artifacts remain useful, but they are not substitutes
  for a real todo/task source:
  - `.claude/latest-prompt.txt`
  - `.claude/notifications/*.json`
  - `.claude/review-notes.md`
  - repo/worktree plan files such as `PLAN.md` / `.claude/plan.md`
- `PLAN.md` and related plan-mode files may still provide useful context, but
  they are not the same structured task state Claude is already using.

Current recommendation:

1. keep the sidebar work that reuses existing metadata separate from this
   instrumentation step
2. implement current-task support first by reading Claude's own `tasks/`
   store keyed by `claude_session_id`
3. add hook handling for `TaskCreate` / `TaskUpdate` so sidebar state can
   refresh immediately instead of waiting for transcript/file polling
4. treat the real todo list as a second investigation:
   - first try to validate whether `TodoWrite` can be hooked cleanly and
     whether `~/.claude/todos/` is reliable enough to read directly
   - if that proves unreliable, add an AMF-managed local sidecar for todos
     rather than scraping transcripts

## Testing Plan

There are already resize-oriented tests in `src/app/tests.rs`, but there are no
real ratatui render tests yet. Add:

- a pure layout test for sidebar width math
- a render test for "Claude view shows blank sidebar"
- a render test for "non-Claude view stays full width"
- an updated resize expectation so Claude panes are resized to `content_width`,
  not terminal width

## Recommended Delivery Order

1. Ship the blank-sidebar checkpoint first.
2. Review the feel in a real Claude session.
3. Wire in existing metadata.
4. Add latest-prompt context with timestamp.
5. Add live activity detail from thinking/tool hooks.
6. Add waiting/input detail from pending notifications.
7. Polish session / feature metadata layout and wording.
8. Add the sidebar UI follow-up pass:
   - expandable section keybinds
   - remove `Session`
   - remove `Summary`
   - improve todo auto-scroll behavior
9. Investigate and design a real todo list.
10. Investigate and design a current-task model.
11. Decide whether richer Claude-side structured data is worth building later.
