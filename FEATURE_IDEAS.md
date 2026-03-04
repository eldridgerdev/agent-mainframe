# AMF Feature Ideas

## Session Intelligence

- **Auto-summarize sessions** - When a Claude session goes
  idle, capture its last output and generate a one-line
  summary shown in the dashboard (e.g., "Refactored auth
  module, waiting on test fix")
- **Session activity sparkline** - Show a tiny sparkline
  graph next to each feature showing activity over the
  last hour (keystrokes/output bursts)
  - tokens used?
- **Cross-session context sharing** - Let one session
  reference another's work via a shared scratchpad file
  that all sessions in a project can read/write
- **Plan Mode feature** - A feature that is just meant to build
  a plan from plan mode. Will spawn new subworktrees (automatic/manual?) for
  agents to build from with the shared plan.

## Workflow Automation

- **Pipeline chains** - Define a sequence like
  "lint → build → test" that runs across sessions
  automatically, stopping on failure
- **Session templates** - Save a set of initial
  prompts/configurations as a template (e.g., "bug-fix
  template" that pre-loads relevant files and
  instructions)
- **Scheduled prompts** - Queue up prompts to send to a
  session at a specific time or when it goes idle

## Multi-Agent Coordination

- **Diff review between sessions** - Compare the changes
  two different feature sessions have made to the same
  files, highlighting conflicts before they become merge
  conflicts
- **Supervisor mode** - A meta-agent that watches all
  active sessions and takes action when things go wrong.
  - **Detection layer** - Periodically captures pane
    output from all active sessions (reusing existing
    `capture_pane` infra) and looks for signals:
    - Session idle for a long time with no commits (stuck)
    - Claude in a retry loop or repeating the same error
    - A permission prompt sitting unanswered
    - Build/test failures unresolved after multiple
      attempts
  - **Dashboard status icons** - Beyond Active/Idle/Stopped,
    show supervisor-generated states on each session:
    - "stuck" - no progress in N minutes
    - "needs input" - waiting on a prompt or question
    - "looping" - repeating similar output patterns
    - "conflict" - editing files another session changed
  - **Actions when flagged** - Options for responding to
    detected issues:
    - Send a nudge prompt to the stuck session ("try a
      different approach")
    - Reassign the task to a new session with a refined
      prompt
    - Pause and surface it to the user for manual
      intervention
    - Auto-answer permission prompts based on configured
      policy
  - **Cross-session awareness** - Higher-level coordination:
    - Detect two sessions editing the same files and warn
      early
    - Notice when one session's fix could help another
      (e.g., session A fixed a build issue session B is
      also hitting)
    - Aggregate progress into a summary ("3/5 features
      progressing, 1 stuck, 1 waiting for input")
- **Session forking** - Clone a session's state into a new
  session to try an alternative approach, then pick the
  winner

## UX Improvements

- **Session tagging/filtering** - Add colored tags (e.g.,
  `#urgent`, `#blocked`, `#review`) and filter the
  dashboard by tag
- **Split view** - Show two sessions side-by-side in the
  TUI instead of only one at a time
- **Session timeline** - A horizontal timeline view showing
  when sessions were active/idle/stopped over the day
- **Quick notes** - Attach sticky notes to features visible
  in the dashboard without entering the session
- **Harpoon-style session bookmarks** - Pin up to 4-5
  sessions to numbered slots (like harpoon2 for neovim)
  and jump between them instantly with a keybind (e.g.,
  leader+1 through leader+5). Works across projects and
  features so you can bookmark your most active sessions
  regardless of where they live in the tree. Include a
  small UI showing your pinned slots (e.g., in the status
  bar or a quick overlay). Support reordering pins,
  swapping slots, and clearing slots. Pins persist across
  restarts so your favorites survive session changes

## Mouse Support

Currently: scroll wheel navigates the project list,
single-click selects items, double-click expands/collapses
or enters view, and a few header elements are clickable.

- **Fix click and drag** - Makes it hard to copy/paste sometimes
- **Scroll wheel in view mode** - Forward scroll events to
  the embedded tmux pane so you can scroll through Claude's
  output without switching to leader key commands
- **Right-click context menu** - Show a popup menu with
  actions relevant to the clicked item (start/stop/delete
  feature, rename session, copy branch name, etc.)
  reorder them in the list
- **Drag to resize split** - If split view is implemented,
  drag the divider between panes to resize
- **Mouse hover highlights** - Highlight the row under the
  cursor to make it obvious what will be clicked
- **Click status indicators** - Click on the
  Active/Idle/Stopped status badge to toggle start/stop
- **Click-to-collapse chevrons** - Add explicit
  expand/collapse chevron icons on projects and features
  that respond to single-click (instead of requiring
  double-click)
- **Middle-click to close** - Middle-click a feature or
  session to stop/close it (similar to middle-click closing
  browser tabs)
- **Clickable breadcrumbs in view mode** - Click individual
  breadcrumb segments in the view header to navigate back
  to that level (project → feature → session)
- **Tooltip on hover** - Show a brief tooltip with feature
  details (branch, workdir, last accessed) when hovering
  over an item for a moment

## DevOps Integration

- **CI status badges** - Show GitHub Actions / CI status
  next to each feature branch directly in the dashboard
- **Auto-PR draft** - When a feature session goes idle and
  has commits, offer to create a draft PR automatically

## Usage Tracking & Cost

- **API cost tracking** - For API-based subscriptions,
  track actual dollar spend per session by parsing Claude
  API responses for token counts and multiplying by
  per-model pricing. Show running totals per session,
  per feature, and per project
- **Budget limits** - Set a dollar cap per session, feature,
  or project. Warn at 80% and optionally pause the session
  at 100% to prevent runaway costs
- **Cost dashboard** - A dedicated view showing spend over
  time: daily/weekly/monthly bar charts, breakdown by
  project, and trend lines. Highlight which sessions are
  the most expensive
- **Session cost badge** - Show a small cost indicator
  next to each session in the project list (e.g.,
  "$1.24") so you can see spend at a glance without
  entering a detail view
- **Token usage breakdown** - Track input vs output tokens
  separately since they have different pricing. Show
  context window utilization (how full is the session's
  context)
- **Subscription plan awareness** - For Pro/Max plans with
  usage limits, track how many messages or tokens have
  been used in the current billing period and show
  remaining capacity
- **Cost per outcome** - Track cost relative to results:
  cost per commit, cost per feature completed, cost per
  lines changed. Helps identify which types of tasks are
  cost-effective to delegate
- **Export usage reports** - Export CSV/JSON reports of
  usage data for expense reporting or team budgeting
- **Session duration tracking** - Track wall-clock time
  each session is active, time-to-first-commit, and
  total active time per feature for productivity insights
- **Rate limit monitoring** - For API subscriptions, track
  requests per minute and warn when approaching rate
  limits. Queue or throttle sessions to stay under limits
- **Multi-provider cost comparison** - If AMF supports
  other backends (opencode, etc.), normalize costs across
  providers so you can compare apples-to-apples

## Knowledge Management

- **Session log export** - Export a session's full
  conversation history as markdown for documentation
- **Lessons learned DB** - Automatically extract patterns
  from completed sessions (what worked, what didn't) into
  a searchable local database
