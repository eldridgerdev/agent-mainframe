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
4. Investigate Claude hooks and adjacent AMF features to find the best source
   of richer sidebar data.
5. Decide whether richer Claude-side structured data is worth building.
