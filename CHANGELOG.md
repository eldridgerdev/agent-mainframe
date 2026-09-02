# Changelog

All notable changes to AMF are documented in this file.

This changelog follows a Keep a Changelog style layout. Use
`## [Unreleased]` for pending work, then add a dated release section
when cutting a version. Major and minor releases are expected to
document user-facing changes and any migration notes here before they
are tagged.

## [Unreleased]

### Fixed

- **PR Triage fix prompts can now be reviewed and redirected without editing.**
  The injection confirmation dialog now scrolls long prompts with `Ctrl+J/K`
  or `Page Up/Down` before edit mode is entered. Press `t` to return to the
  destination picker and choose another live, dedicated, or companion session;
  any edits already made to the prompt are retained. No migration is required.

- **The Active TODO sidebar now keeps its completion shortcut visible.** When
  an agent session is associated with a TODO, its `Active TODO` box shows
  `Ctrl+Space`, then `z` in the header just like the Prompt and Plan boxes
  show their shortcuts, including in narrow sidebars. No migration is
  required.

- **Pasting into the native TODO editor now works reliably.** Pasted text is
  inserted into the active TODO field without submitting the dialog. TODO
  titles remain single-line, while notes and scratchpads retain pasted line
  breaks. No migration is required.

### Added

- **PR Triage can investigate a review comment instead of fixing it.** A
  comment that asks a question rather than requesting a change can be routed to
  a strictly read-only headless investigation: press `v` to toggle the selected
  comment between *fix* and *investigate* (default stays *fix*), then `f` to run
  it. The investigation gets minimal context — the comment, the PR title and
  description, and the list of changed files, with no file contents — picks its
  harness per run, and blocks the overlay until it returns; it inspects the
  repository but cannot edit files, run commands, or write anything. The answer
  persists per pull request and reopens with the triage overlay, shown in the
  detail panel with its status, harness, and time. Press `a` on a finished
  investigation to act on it: convert it back to a fix, add it to the batch,
  post an editable reply, ask a follow-up (re-runs read-only with the prior
  answer as context), dismiss it, or keep it as a TODO. Investigate is
  single-item and never joins a batch fix. This adds a `pr_investigations`
  table; existing databases upgrade automatically with no change to other data.

- **The native TODO editor can now use Vim keybindings.** Every inline edit
  in the scoped-TODOs overlay — add, edit title, edit notes, and the
  scratchpad — takes `Ctrl+T` to toggle the Vim keymap, matching the Compose
  box and the final-review editors. The choice is remembered for the life of
  the overlay (a fresh overlay starts with Vim off), a freshly opened Vim
  editor lands in Normal mode, and `Ctrl+Q` cancels an edit since Vim's `Esc`
  switches Insert→Normal instead. The hint line under the editor shows the
  active mode. Requires no migration.

- **A TODO can now start an agent in a brand-new feature without plan
  mode.** Pressing `Enter` on an unlinked TODO used to offer two choices —
  "Start an agent on this TODO" (which, for a project- or global-scoped
  TODO, could only pick an *existing* feature) and "Plan this TODO first".
  Getting a fresh branch and worktree for a TODO therefore meant sitting
  through the discovery interview. The chooser now has a third option,
  "Start an agent in a new feature": it opens the normal create-feature
  wizard pre-filled with a branch name from the TODO title and plan mode
  left off, and once the feature exists it links the TODO to it and seeds
  that feature's agent with the TODO, unsent. It declines with a reason on
  a project that is not a git repository. Requires no migration.

- **AMF's own AI review (`W`) now carries model, token, and cost
  attribution.** After a run completes, the AI Review pane shows a line
  naming the harness and model that produced the findings plus the run's
  input/output tokens and estimated cost. The same disclosure is inserted
  above the `— AI review via AMF` marker on the posted GitHub summary and on
  every inline comment, so a reader on the PR can tell which model reviewed
  their code and roughly what it cost. It uses the same configured pricing
  and rounding as AMF's other usage meters, and matches the disclosure
  already shown on AI-drafted PR Triage replies. A harness that reports no
  token usage degrades to model-only attribution rather than showing a
  fabricated `$0.00`; a review generated before this change keeps the bare
  marker.

### Changed

- **PR Triage now attributes a combined batch fix's cost to every comment it
  resolved, and marks it as shared.** When you fix several review comments in
  one `B` batch, the single agent run's cost used to be invisible per issue.
  Each resolved comment in the batch now discloses the *whole run's* cost —
  relabelled `Fix cost (est.):` in the reply dialog and in the reply posted to
  GitHub — followed by `· combined (N)` and, in the posted text, a plain line
  explaining it was one of N comments handled in a single run and the figure is
  shared. In the comment list, batched comments carry a `⧉` marker that
  brightens on the siblings of whichever comment is selected, and `[` / `]`
  jump between them. The badge and cost appear only on comments that were
  actually resolved; a batch comment you never replied to shows nothing.
  AMF's own AI Review (`W`) shows the same `Fix cost (est.): … · combined (N)`
  on a finding once it has been posted and then fixed in PR Triage as part of
  a batch, matched back by file and line. Single-comment fixes are unchanged,
  and triage records written before this release are left as-is.

- **The final review's `t` key now picks where fixes are applied, not just
  live-vs-dedicated.** It used to flip silently between the feature's own agent
  session and a fresh dedicated "Final Review" session. `t` now opens a
  destination picker — modelled on PR Triage's — with four choices: this
  feature's live session, a dedicated review session on a harness you pick,
  **another existing feature's** agent session, or **a brand-new companion
  feature** (its own worktree branched from the feature under review, with its
  own harness and vibe mode). The companion keeps its fixes isolated; landing
  them back on the source branch is an explicit step — press `t` on that
  feature's dashboard row to push or cherry-pick its commits. The footer target
  label and the review `?` help overlay reflect the new options. Reviews in
  progress are unaffected; the default target is still this feature's live
  session.

- **Planning a TODO into its own feature no longer proposes a sentence-long
  branch name.** TODO titles are written as sentences, and the create-feature
  wizard seeded the branch with the whole thing slugified — which then became
  the worktree directory name, the tmux session name, and the dashboard row.
  The seeded name is now shortened to at most 32 characters, cut on a word
  boundary so it still reads as a name rather than a truncation. It is still
  only a seed: the wizard opens on the branch field, so a name worth spelling
  out in full is one keystroke away.

- **The agent sidebar now always shows the viewed session's context usage,
  not only when it is nearing the limit.** The section (renamed from
  `Fresh Context` to `Context`) appears for every Claude, Codex, or opencode
  session that has a reading, in every band — a calm green line like
  `Ctx 42% · 42,000` while there is headroom, turning amber then red as
  pressure rises. At the warning or critical threshold the same section still
  adds `Ctrl+Space`, then `F` to open a new session in the same feature with
  an editable continuation prompt; `Ctrl+Space`, then `X` dismisses that call
  to action while leaving the reading in place, and it returns after the
  session clears or resets. Estimated and stale readings stay labeled, while
  unavailable or reset-pending readings do not invent a percentage. No
  migration is required.

### Migration

- The AI review attribution adds an optional field to the existing
  `ai_review_cache` JSON rows; older cache entries load unchanged and simply
  show no attribution until the next run.

- Schema migration 032 adds two nullable columns (`batch_id`,
  `batch_fix_cost`) to `pr_comment_triage`. It applies automatically on
  startup; existing triage rows keep both as `NULL` (not part of any batch)
  and are unaffected. No downgrade step is needed — an older AMF simply
  ignores the columns.

## [v0.41.0] - 2026-08-27

### Added

- **The embedded session sidebar now shows a Usage box.** Directly under
  Status, it lists the current harness's account-level rate-limit windows —
  the same 5h/7d figures the dashboard status bar shows — so you can check
  your remaining headroom without leaving the session. It appears once the
  usage numbers are known and is left out for harnesses that don't report
  usage (OpenCode, Pi).

- **Plan-mode multiple-choice questions now take your own free-text answer.**
  Every select question in the guided plan interview shows a "Your own answer"
  box beneath the options. Press `e` to type into it; you can answer purely
  with your own text, or pick option(s) *and* add elaboration. `Enter` in the
  box commits it and returns focus to the option list without submitting the
  question; `Esc` discards the edit; `Enter` on the option list still submits.
  Press `Backspace` to clear a pick and answer with custom text alone.
  There is a 500-character limit (multi-line allowed) shown as a `used/500`
  counter. A question with nothing picked and a blank custom answer stays
  unanswered, exactly as before. The submitted answer is a single plain
  string — picked labels, then ` — ` and your text — so the AI rounds and the
  saved plan treat it like any other answer, and revisiting the question
  restores the selection and the custom text rather than a flat string. Visual
  proof regenerable via
  `scripts/dev/screenshot/scenarios/plan-interview-custom-answer.txt`.

- **TODO-launched agent sessions now keep their own TODO visible in the agent
  sidebar.** The box shows only that TODO's title and whether it is open or
  completed, and it remains visible after completion. From the embedded
  session, press `Ctrl+Space`, then `z` to confirm completion. References
  follow TODOs when they move between scopes and are removed when an item is
  deleted. No migration is required; existing sessions simply have no
  reference until launched from the TODO menu.

- **The agent sidebar now always shows the viewed session's context usage.**
  A `Context` section appears for every Claude, Codex, or opencode session
  that has a reading, in every band — a calm green line like
  `Usage: Ctx 42% · 42,000` while there is headroom, turning amber then red
  as pressure rises. At the warning or critical threshold the same section
  adds `Ctrl+Space`, then `F` to open a new session in the same feature with
  an editable continuation prompt; `Ctrl+Space`, then `X` dismisses that call
  to action while leaving the reading in place, and it returns after the
  session clears or resets. Estimated and stale readings stay labeled, while
  unavailable or reset-pending readings do not invent a percentage. No
  migration is required.

- **Harness pickers now show how much rate-limit headroom you have left,
  right where you choose a harness.** When creating a feature (Harness
  step) or adding a session (`s`), a Claude or Codex option now shows a
  line underneath it like `5h 62% left · resets in 3h   7d 90% left`, so
  you can see you're about to run low before you start. It reads the same
  cached numbers already shown in the dashboard's status bar — no extra
  waiting, no new login prompts. If nothing is known for a harness (this
  includes OpenCode and Pi, which don't expose this today), the line is
  simply left out.

- **Final-review PR-triage comment and suggestion editors can now use Vim
  mode for the whole review session.** Press `Ctrl+T` to toggle it for every
  editor; enabling starts in Normal mode, while `Tab` submits and `Ctrl+Q`
  cancels from either keymap. Vim mode starts off for each new review, so
  existing plain-editor behavior is unchanged.

- **AMF's own AI review (`W`) now carries model, token, and cost
  attribution.** After a run completes, the AI Review pane shows a line
  naming the harness and model that produced the findings plus the run's
  input/output tokens and estimated cost. The same disclosure is inserted
  above the `— AI review via AMF` marker on the posted GitHub summary and on
  every inline comment, so a reader on the PR can tell which model reviewed
  their code and roughly what it cost. It uses the same configured pricing
  and rounding as AMF's other usage meters, and matches the disclosure
  already shown on AI-drafted PR Triage replies. A harness that reports no
  token usage degrades to model-only attribution rather than showing a
  fabricated `$0.00`; a review generated before this change keeps the bare
  marker.

### Fixed

- **A TODO started through plan mode now shows its "Active TODO" box in the
  agent sidebar.** The two direct spawn routes ("start an agent on this
  TODO" and "start an agent in a new feature") tagged the launched session
  with its originating TODO, but the two plan-mode routes — "plan this TODO"
  in the host feature and in a new feature — only recorded the
  feature-level link. The planned agent's sidebar therefore never showed
  which TODO it was working, and `leader z` had nothing to complete.
  Both plan routes now attach the same session reference, so the sidebar
  section and its completion hint appear regardless of how the agent was
  launched. The new-feature plan route also records the session on the
  TODO's work state, matching the non-plan route.

- Fixed the new usage line above being unreadable when its row was
  selected in the add-session picker, in themes (including the default)
  where the selection highlight and the line's muted text color matched.
  It now switches to the brighter selected-row text color.

### Migration

- Schema migration 030 runs automatically on first launch, adding a
  `custom_answers` column to the `plan_interviews` table so custom
  plan-interview answers survive a resumed or re-run interview. Existing
  rows backfill to "no custom answer"; no user action is required.
- The Vim toggle is transient and is not persisted.

## [v0.40.0] - 2026-08-26

### Added

- **A new leader command starts a fresh agent session for continuing work
  without dragging along a long conversation history.** From an agent
  session, press `Ctrl+Space`, then `Shift+F`. AMF asks what you want the new
  session to do, then opens a brand-new session in the same feature, using
  the same agent. Its compose box arrives pre-filled (not sent) with your
  instruction plus a pointer to the feature's current plan and the files
  changed on this branch, so it starts oriented without inheriting the old
  session's accumulated context — review it and send when ready. If the
  feature has no plan file, that part is left out and AMF says why; on a
  non-git project, or a branch with nothing changed yet, the changed-files
  list is simply left out too.

- **You can now customize the context-window size and the warning/critical
  thresholds behind the `Ctx` indicator, instead of relying on AMF's
  hardcoded defaults.** Press `w` on the dashboard to open Context Window
  Settings: set a token count to override the context-window size AMF
  assumes when a harness doesn't report its own (useful if you're on a
  larger or smaller context-window plan than AMF's default guess), and set
  the usage percentages at which the indicator switches to `WARNING` and
  `CRITICAL` (70% / 85% by default). These are global settings, saved to
  your AMF config. No migration is required — leaving both blank/default
  keeps today's behavior exactly as it is.

### Changed

- **Starting a TODO plan now marks the item in progress immediately.** Choosing
  **Plan this TODO first** changes the TODO from `[ ]` to `[~]` before the plan
  destination is selected, so the list accurately shows that planning work has
  begun and `I` will not assign the same item again. Cancelling later plan or
  feature setup keeps the item in progress; failed direct agent launches still
  roll back to not started as before.

- **The context-window indicator now shows the raw token count, not just the
  percentage.** Every session row's `Ctx` indicator — Normal, `WARNING`, and
  `CRITICAL` alike — now reads its actual token count next to the label
  (e.g. `Ctx ~91% CRITICAL · 182,000`), so you can judge how large a
  session's context has actually grown instead of relying on the severity
  label alone. No config changes or migration required.

- **Raised the fallback context-window size used for Claude Code sessions
  from 200,000 to 900,000 tokens.** This only affects the `Ctx` percentage
  when Claude's own status line or transcript doesn't report its context
  window size directly; AMF's estimate now better matches Sonnet's actual
  auto-compact window instead of understating it.

- **Project and global TODO lists can now be shown or hidden independently.**
  Press `p` for the project list and `g` for the global list; hidden scopes stay
  discoverable through labeled placeholders and are excluded from pane
  navigation, `I` (implement next), and other cross-pane actions. The worktree
  list remains visible whenever the feature has one, while repo-root features
  may hide both optional lists. Visibility is shared by every TODO view for the
  current AMF run and resets to both lists shown on the next launch.
- **TODO priority and launch keys moved to make room for the scope toggles.**
  Press `P` to cycle priority and `Enter` to start or plan the selected TODO.
  The previous `\` side-pane toggle and `g` launch alias are no longer used in
  the TODO editor.

### Fixed

- **Deleting a feature or project now clears its remembered PR association.**
  Reusing the same branch name for later work no longer shows the old feature's
  merged or closed PR badge. Failed deletions leave the association intact.

- **Accepting a completed plan now asks immediately before starting above the
  agent concurrency limit.** The completed plan stays available while the
  Resource Check popup is open: continue to create and start the planned
  feature with its original kickoff prompt, or cancel back to plan review
  without creating it. This replaces the detour to the dashboard and the
  follow-up “Press c to start it” warning.

### Migration

- No migration is required for PR cleanup. Associations are cleared when a
  feature or project is deleted after upgrading.
- No migration is required. Existing TODO states and associations are
  preserved.
- No data migration is required. TODO contents and pane state are retained when
  a scope is hidden; only the TODO editor keybindings changed.
- No migration is required for plan completion confirmations. Existing agent
  limits and plan-mode features continue to use their current configuration.

## [v0.39.0] - 2026-08-25

### Added

- **Agent sidebars now keep the feature's current plan within reach.** Claude
  Code, Codex, OpenCode, and Pi sessions show the current plan in a dedicated
  sidebar section; press `Ctrl+Space`, then `n` to open it in AMF's read-only
  Markdown viewer. `AMF_PLAN.md` is selected automatically. When it is absent,
  AMF offers Markdown files from the feature worktree and remembers the chosen
  file for that feature. Moved or deleted selections are cleared safely, and
  `r` refreshes a plan while it is open. Dashboard and non-agent session rows
  remain unchanged.

- **Agent session rows now show context-window pressure before the next prompt
  runs out of room.** Claude Code, Codex, OpenCode, and Pi sessions display a
  compact `Ctx` percentage that turns yellow with `WARNING` at 70% and red
  with `CRITICAL` at 85%. Estimated readings are marked with `~`, stale
  readings are labeled, and detected compaction or a new conversation clears
  the old value until fresh usage arrives. Terminal, editor, TODO, and custom
  rows remain unchanged.

- **The dashboard PR badge now shows merged and closed pull requests, not just
  open ones.** Once a feature's PR is merged or closed without merging, its
  badge switches to `[PR #N merged]` / `[PR #N closed]` instead of vanishing,
  so a finished feature stays visually distinct from one that never had a PR.
  This applies to the feature row and to the badge shown in an embedded
  session's header/sidebar. Requires an authenticated `gh` and a GitHub
  remote, same as the existing open-PR badge, and refreshes on the same
  background schedule — no extra `gh` calls beyond what a branch actually
  needs. No migration is required; the database updates itself on first
  launch after upgrading.

### Changed

- **TODO assignment now has a durable three-state lifecycle.** Items are
  explicitly **not started**, **in progress**, or **completed**, with `[ ]`,
  `[~]`, and `[x]` markers in the TODO editor. Starting a TODO-specific agent
  reserves the item as in progress before launch, prevents a second agent from
  being assigned to the same item, and makes `I` continue to the next
  not-started TODO. If session creation or composer setup fails, the reservation
  rolls back so the item can be tried again.

  Closing or stopping the associated agent does not silently reset the work:
  the TODO stays in progress until you change it. Press `Space` or `x` to cycle
  not started → in progress → completed. If an associated session is removed,
  AMF clears the stale link while preserving the visible in-progress state.

### Fixed

- **AI Review findings now stay attached to the correct side and source line
  across multi-hunk diffs.** Review prompts label current-file (`RIGHT`) and
  deleted base-file (`LEFT`) lines explicitly, so earlier additions or removals
  no longer shift later findings onto a different row. Deleted-line findings
  show `(base)`, and an ambiguous or invalid location falls back to a file-level
  finding instead of displaying or posting a misleading line number.

- **PR Triage now confirms when an AI Review completed with no findings.**
  The header shows `[AI review: no findings (<age>)]` for the current PR
  revision instead of looking identical to a review that never ran. Running
  reviews and unpublished findings still take precedence, while failed reviews
  get a separate failure badge without crowding the header with error details.
  Same-revision results survive leaving the pane and restarting AMF; a new head
  commit starts unreviewed as before. Empty or incomplete agent responses are
  recorded as failures rather than clean reviews.

### Migration

- No migration is required for current-plan shortcuts. Existing features start
  without a manual selection, and AMF updates its database automatically.
- No migration is required for context-window indicators.
- No migration is required for AI Review line mapping. Existing cached findings
  remain readable; entries without side information are handled conservatively
  until the review is regenerated.
- No migration is required. Existing same-revision AI Review cache entries
  remain readable and expire under the existing one-week retention policy.
- No manual TODO migration is required. Existing completed items remain
  completed; existing incomplete items begin as not started with no agent
  association.

## [v0.38.0] - 2026-08-21

### Added

- **TODO lists are scoped to the work they belong to.** A feature's TODO editor
  now opens on that worktree's own list rather than one shared list per
  project, and every feature can have its own `TODOs` session instead of the
  first one claiming it for the whole project.

  Two other scopes sit beside it. Press `\` in the editor to reveal the
  **project** list — the one that existed before — and a new **global** list
  that belongs to no project and is shared across every repo AMF knows about.
  The three panes each keep their own cursor, scroll, and scratchpad; `Tab` /
  `Shift+Tab` move between them, and the reveal is remembered between sessions.
  The global list has no entry point of its own — it is reachable as a side
  pane of any TODO editor.

  `M` moves the selected TODO to another scope and `C` copies it. A move
  carries the item's session and planned-feature links with it, because it is
  the same work; a copy deliberately lands unstarted, so two panes never both
  claim the same agent.

  `I` (implement next) now scans whichever lists are showing: the worktree list
  alone with the side panes closed, all three with them open. Priority still
  comes first, and at equal priority the narrower scope wins — worktree, then
  project, then global.

  A worktree TODO is still worked in the feature that owns the checkout. A
  project or global TODO belongs to no one checkout, so `g`, `Enter`, and `I`
  now ask which feature should work it; that feature supplies the agent and
  permission mode exactly as before.

  `Ctrl+Space` `N` quick-capture writes to the session feature's worktree list
  (the project's when the feature sits on the repo root), and the capture box
  names the list it is writing to.

  Deleting a feature deletes its worktree list along with the checkout, so if
  that list still holds unfinished items AMF asks first: move them to the
  project list, move them to the global list, delete them with the worktree, or
  cancel. Nothing is killed or removed until you answer.

  **Migration:** existing TODO lists are untouched. They stay project-scoped,
  keep their host feature, their scratchpad, and their session links, and
  appear in the project pane. New worktree lists start empty. Deleting a
  project still removes its lists — now its worktree lists as well as its
  project list — and never the global one.

- **`I` starts the next TODO on the list.** Working a TODO list meant picking
  an item, pressing `g`, and remembering where you were — so a list that is
  already in priority order still had to be read before it could be worked.
  `I` takes the highest-priority TODO nobody has started, opens an agent on it
  in the list's feature with the composer seeded and unsent (exactly as `g`
  leaves it), and marks the item **in progress** so the next `I` moves on.
  Priority decides the order; within one priority, the order you arranged the
  list in breaks the tie.

  It works from two places: inside the TODOs list, and on the `TODOs` row on
  the dashboard, so you can start the next piece of work without opening the
  list at all. On any other row the key does nothing — it means "take the next
  item off *this* list", and there is no list to guess at.

  In-progress items show as `[~]` and are skipped by the scan. `i` on a TODO
  sets or clears that mark by hand, for when you closed a session's window
  rather than the session; completing an item clears it too, and a TODO whose
  agent session has since been removed is picked up again automatically rather
  than staying marked forever. When every remaining TODO is already underway,
  AMF offers the next one anyway and asks whether to go to the work already
  started, start a second agent on it, skip it, or cancel — rather than
  reporting an empty list that is not empty.

- **A TODO can start a plan interview, in place or in a new worktree.** `g`
  (or `Enter`) on a TODO now asks whether to start an agent on it — the
  previous behavior — or to plan it first. Choosing to plan asks where the
  plan should land: the feature the list lives in, or a new feature and
  worktree created for it. The feature brief arrives pre-filled from the
  TODO's title, notes, and the list scratchpad, and is editable before the
  first question. Picking a new feature opens the ordinary create-feature
  wizard pre-filled from the TODO, so the branch, agent, and permission mode
  are still yours to change. Either way the accepted plan is saved and an
  agent is started on it.

  A plan that lands in a feature that already exists is written to
  `AMF_PLAN.todo-<name>.md`, beside that feature's own `AMF_PLAN.md` rather
  than over it, and the agent is told which file is its own. A TODO planned
  into a new feature stays open on the list, is marked as linked, and `g`
  afterwards jumps to that feature instead of asking again; if that feature is
  deleted the TODO survives, the link is dropped, and the choice is offered
  again. An interrupted interview is kept as a draft and offered back the next
  time you press `g` on that TODO.

- **PR Triage can run multiple named dedicated sessions at once.** After
  choosing a dedicated harness from the first `f` or `B` fix-target prompt,
  enter a session name to create or reuse that exact triage session. Leave the
  name blank to keep reusing the original `PR Triage` session. The chosen name
  follows fix injection, session switching, activity, and usage reporting.

- **Plan interviews can pause while you inspect the codebase.** Press
  `Ctrl+Q` to park the interview on the dashboard without losing the answer
  you are editing. AMF marks the relevant project or feature row so `Enter`
  can resume it directly; if you open a session to investigate first,
  leaving that session returns to the interview automatically. A plan
  operation already using agent tokens must finish before the interview can
  be parked, so its result is not discarded.

### Changed

- **Project config moved to `amf.json` at the repo root.** AMF's per-project
  settings — custom sessions, feature presets, lifecycle hooks, keybindings,
  plan questions, and prompt templates — now live in a tracked `amf.json`
  beside `Cargo.toml`, instead of `.amf/config.json`. Committing config no
  longer needs `git add -f`: `.amf/` is ignored dir-wide because everything
  still inside it is generated, and the config file is no longer an exception
  hidden in an ignored directory. Existing `.amf/config.json` files keep being
  read until the next config write moves them, and that move either completes
  or leaves the old file untouched — an interrupted migration never leaves you
  with a half-written config.
- **`amf doctor` reports projects still on the legacy config path.** The new
  `config-path` finding names each `.amf/config.json` it can still see, and
  distinguishes the two cases: a file that is simply not migrated yet (still
  read, and moved on the next config write) from one sitting next to an
  `amf.json` that has already superseded it, where edits to the old file do
  nothing.
- **`.amf/` explains itself.** The directory now gets a `README.md` when AMF
  creates it, saying that its contents are generated and safe to delete, and
  pointing at `amf.json` for real settings. Your own edits to that README are
  left alone.
- **The dashboard's needs-attention badge now shows `<leader i>`.** Questions,
  completed work, waiting sessions, and other input requests now advertise the
  shortcut that opens the list, so the action is discoverable without first
  opening help. On a narrow terminal the working directory is shortened before
  the badge is, so the shortcut stays readable. No configuration change is
  required.
- **Plan mode now keeps its approved plan in `AMF_PLAN.md` at the feature root.**
  The plan is no longer hidden under the Claude-specific `.claude/` directory,
  and the AMF-specific name avoids overwriting a repository's conventional
  `PLAN.md`. Codex also receives the same editable, unsubmitted kickoff prompt
  as Claude when a newly approved plan starts work, including when startup
  steering was enabled. Existing `PLAN.md` and `.claude/plan.md` files remain
  readable as fallbacks.

### Fixed

- **AMF no longer exhausts your GitHub API budget refreshing PR badges.** The
  dashboard's pull-request badge was refreshed every 30 seconds, and each sweep
  made a GitHub API call for *every* feature you have — not just the ones with
  open pull requests. On a workspace with ~34 features that is around 8,500
  points an hour against GitHub's 5,000-point hourly GraphQL budget, so simply
  leaving AMF open drained the whole allowance in about 35 minutes. The first
  visible symptom was usually PR Triage failing with a rate-limit error — the
  one workflow that had not caused it.

  Two changes fix it. The sweep now makes **one request per repository** rather
  than one per feature: a single query returns every open PR in a repo with its
  unresolved-thread count, and each feature's branch is matched against that
  locally. A project with thirty worktrees now costs the same as one with a
  single feature. Badges also refresh every 5 minutes instead of every 30
  seconds, which is far inside a PR badge's useful freshness. The longer
  interval governs how often badges refresh, not how long you wait to see one:
  the first sweep still runs a few seconds after launch.

  When GitHub does report an exhausted budget, AMF now says so and pauses badge
  refresh for 15 minutes rather than retrying into an empty allowance, and
  features it did not get to keep their existing badge instead of blanking.

  Two smaller behavior changes come with the batching: a repository with no
  GitHub remote is now treated as "no pull requests" instead of reporting an
  error per feature, and a feature is matched by the branch AMF has recorded for
  it rather than by whatever branch its worktree currently has checked out. Note
  that a worktree whose `origin` is a **fork**, with its PR on the upstream
  repository, no longer shows a badge.

- **A waiting session is one entry in the needs-attention list, not hundreds.**
  Harnesses re-report a stop freely — Claude's Stop hook fires at every turn
  boundary — and each report was queued as its own input request, so a session
  left waiting could fill `i` (and the header count) with identical rows for
  the same feature. Only one wait is now pending per session, showing what the
  session last said; a re-report replaces it silently instead of toasting
  again. Diff reviews and change reasons are unaffected — each one is its own
  request about its own edit and still gets its own row.

- **Features with a damaged worktree can be deleted again.** If a worktree's
  `.git` file is already missing, Git refuses to remove it even with force and
  AMF used to leave the feature permanently stuck in the dashboard. AMF now
  recognizes that stale-worktree failure and finishes removing the feature.

### Migration

- **Project config migrates itself.** `.amf/config.json` is still read, so
  existing checkouts keep working untouched. The next time AMF writes config
  — saving the config wizard, exporting a prompt template — it writes
  `amf.json` and removes the old file, so there is only ever one answer to
  where config lives. To migrate by hand, `git mv .amf/config.json amf.json`.
  Drop the force-tracked `.amf/config.json` exception from your `.gitignore`
  once you have.
- No migration is required for plan mode. The next plan you accept is written
  to `AMF_PLAN.md`; older `.claude/plan.md` files may be removed when no longer
  needed.
- Existing `PR Triage` and legacy `PR Review` sessions continue to be
  recognized when the default name is used.

## [v0.37.0] - 2026-08-18

### Added

- **The dashboard says why an agent stopped.** A stopped session used to be a
  single undifferentiated "waiting for input", which is the one thing you
  already knew. AMF now reads its harnesses' lifecycle hooks and marks each
  stopped session as **Question** (the agent asked something, or wants
  permission, and cannot continue without an answer), **Completed** (its turn
  ended and the work wants a look), or **Waiting** (stopped, reason unknown).
  The state shows on the feature's row and in a header count broken out by
  kind — with anything the states cannot explain, such as a diff review or a
  change reason, still counted alongside them — and `i` is now one
  needs-attention list — questions first, oldest
  first within each group — replacing the flat input-request picker. Inside a
  session, `Ctrl+Space` then `i` opens the same list and the sidebar reports
  the session's own state. AMF does not capture the agent's message; open the
  session to read the question or the summary.

  A state clears when the agent produces output again, or ages out after
  `waiting_stale_minutes` (default 30; `0` keeps states until the agent
  resumes). States are held in memory only, so an AMF restart clears them and
  sessions show as ordinary active until their next event. Ageing out changes
  nothing about the session itself: a waiting session still counts toward
  `max_concurrent_agents` and still qualifies as dormant under `z`.
- **Learning Mode says when a past question's code has moved.** A question
  remembers the lines it was asked about, and until now nothing checked them
  again — so editing a file left every earlier entry pointing at whatever had
  since taken those line numbers, with no way to tell. That bites the notes
  Learning Mode is most for: an explanation you keep is exactly the one you
  come back to months later, and a stale anchor doesn't look stale, it looks
  like an answer that was always wrong. Reopening a project now checks each
  stored question against the code as it is now. An entry whose code moved is
  marked **moved**, and opening it says where the code went; one whose code is
  gone — rewritten, or in a deleted file — is marked **anchor lost**. Code that
  now appears in more than one place is reported lost rather than guessed at.
  The question and answer are never touched, and neither is the range the
  question was asked at. Handing a moved entry to a live agent (`S`) or keeping
  it as a to-do (`a`) carries the warning along, so neither one sends anybody to
  read the wrong lines.
- **Learning Mode browses the project as a folder tree.** The file list used
  to be every path in the repository, flat and alphabetical, in a pane about
  32 columns wide — so on a real project the first screenful was a run of
  truncated near-identical stubs and `src/` was dozens of keypresses down. It
  is now a tree: folders you can open and close, file names shown without
  their path, and each closed folder saying how many files it holds. It opens
  with the top level visible and the way down to the **Start here** files
  already unfolded, so the entry point is on screen without the rest of the
  repository being. `l` and `h` (or `Enter`) open and close the folder under
  the cursor, `Z` opens or folds every folder at once, and resting on a folder
  changes nothing about the question you have lined up. Very large projects
  are now limited per folder rather than by a single cap on the whole listing,
  and a folder that hides part of its contents says so — which makes the
  all-files view usable on a monorepo instead of merely truncated.

### Changed

- **Harness fidelity varies, and AMF does not guess.** Claude Code and
  OpenCode report all three states. Codex fires one hook when a turn ends and
  cannot say whether it finished or is asking, so its sessions show as
  **Waiting** either way rather than being labelled done. **Pi has no hook
  mechanism at all**, so its sessions carry no state — they behave exactly as
  they did before this release rather than showing a generic waiting marker.
- **Claude workspaces regain a `Notification` hook.** AMF used to strip it,
  because the old wiring queued a pending input for what is only a permission
  prompt. The new wiring runs the attention script alone: it records why the
  session stopped and never touches the notification flow. Only notifications
  that actually block the turn — permission prompts and elicitations — become
  a **Question**; idle nudges, sign-in notices, and completion messages report
  nothing, so they cannot relabel work that has already finished.
  AMF-generated hook scripts live under `~/.config/amf/hooks/` and are
  rewritten on the next startup after upgrading; nothing you wrote yourself is
  touched.
- **AMF's hooks no longer need `jq`.** Every notification hook used to shell
  out to `jq`, and failed quietly without it: nothing errored, the dashboard
  simply never showed an agent working, finishing, or asking — which looks
  exactly like an agent with nothing to say. There is nothing to install now,
  and the hooks work the same on a machine that never had it.

### Fixed

- **A prompt AMF opens a session to deliver no longer disappears when the
  resource warning asks first.** Accepting a plan, spawning an agent from a
  TODO, and escalating a Learning Mode question all open a session and load a
  prompt into its composer for you to review. If the machine was at the agent
  cap or low on memory, the pre-start warning came up in between — and
  answering it opened the session with an empty composer and said nothing, so
  the plan looked handed over when it never was. The prompt now travels with
  the parked start and is loaded once you confirm.

- **The mouse wheel scrolls the plan you are reviewing.** A proposed plan is
  usually taller than the screen, and scrolling it with the wheel did nothing
  — worse than nothing, in fact: the wheel was moving the selection on the
  dashboard hidden behind the dialog, so you could come out of a plan review
  pointed at a different feature than you went in on. The wheel now scrolls
  the plan, the agent's review of it, and the plan and instruction editors,
  three lines at a time like every other scrollable view in AMF. Clicking
  inside the dialog no longer reaches the dashboard underneath either, where
  a double-click could open or start whichever feature happened to be there.

- **Learning Mode says when its file list is incomplete.** In a project that
  is not a git repository, any folder AMF could not open was left out
  silently — and a list missing a whole folder looks exactly like a project
  that does not have one. It now tells you how many folders are missing and
  where to see which ones.
- **Learning Mode's file errors now tell you what to do next.** Opening a
  binary file, a file too large to show, or one you do not have permission to
  read used to stop at the diagnosis. Each message now ends with a way
  forward, and names the file the way the list does instead of printing its
  full path.
- **The scope key rebuilds the file list when it cannot switch scope.** In a
  project that is not a git repository there are no branch changes to switch
  to, so `s` used to only say so. That left the advice for a file that had
  been moved or deleted since the list was built — press `s` to rebuild it —
  doing nothing in exactly the projects it was written for. `s` now rebuilds
  the list in place there, and still explains why the scope did not change.
- **Reopening Learning Mode keeps each conversation together.** Follow-up
  questions are shown indented under the question they continue, but on a
  reopen they were laid out in the order they had been asked — so a follow-up
  asked after a couple of other questions came back indented under whichever
  unrelated question happened to precede it. History now reads on the second
  visit the way it read on the first, and answers you sent back for a deeper
  look stay next to the answer they were checking.
- **Learning Mode failures are recorded.** Files that would not open, folders
  that could not be read, and saved-history problems now appear in the debug
  log (`D` on the dashboard) with the path involved, so a question that went
  nowhere can be traced after the fact instead of vanishing.
- **PR review summaries render cleanly in the Detail pane.** Opening a review
  summary no longer leaves garbled fragments from diff or source lines mixed
  into the review text, and scrolling stays aligned when content wraps.
- **Rejecting a Codex change now actually reverts it.** In vibeless mode,
  turning down a proposed edit left the file on disk exactly as the agent had
  written it: the rejection was recorded and then ignored, so approving and
  rejecting came to the same thing. Rejecting now restores the file, and
  cancelling still leaves it untouched.

### Documentation

- **The README now covers Learning Mode.** A new *Understand a codebase you
  didn't write* workflow — placed first, because it is the one that needs no
  prior knowledge of AMF — explains what the mode is for, that nothing in it
  changes your files and `S` is the single exception, how to start when you do
  not yet know what to ask, the two ask keys and the five keys that act on an
  answer, and the newcomer/familiar reading levels. It also says why `D`
  exists: an ordinary answer only sees the code on screen, so it can name
  files or line numbers that do not exist — and that Codex is the exception,
  since it always reads the repository on the first request, so its answers
  are deep dives already and `D` refuses rather than re-running one. `K` is in
  the dashboard keybindings table.
- **`CLAUDE.md` documents Learning Mode's architecture** — the four new
  modules, the read-only invariant and its one exception, and the distinction
  between a follow-up and a deep dive, which are two different relationships
  between Q&A rows rather than one. Nothing about AMF's behavior changes.

### Migration

No migration is required.

## [v0.36.0] - 2026-08-12

### Added

- **Hand a Learning Mode answer to a live agent.** `S` on an answered entry —
  in the history or while reading the answer — opens an agent session on the
  feature with the composer already filled in: the file and lines you asked
  about, the code itself, your question, and the answer you got. Nothing is
  sent until you send it, so you can edit or delete the whole thing first.
  This is the one way out of the mode's read-only promise, and the message
  says so in its closing line. If the answer came from a run that only saw the
  code on screen, the message asks the live agent to check it against the real
  thing first; at newcomer level it also asks it to explain what it is doing as
  it goes. Entries you have handed over are marked `→ session`, and pressing
  `S` again returns to that conversation rather than starting a second one; if
  that session has since been closed or its agent has exited, a fresh one
  starts and AMF tells you which happened.

### Fixed

- **Lists no longer lose whatever they start with.** Anywhere AMF renders
  markdown — Learning Mode answers, the markdown viewer, plan review, PR
  triage — a bullet that began with code showed only the rest of the line
  (`• — it worked` instead of `` `Ok(())` — it worked``). Task list
  checkboxes vanished for the same reason, so `- [ ] not done` lost its box
  entirely. This hit Learning Mode hardest, because answers written for a
  newcomer explain things in exactly that shape.
- **Learning Mode's answer view no longer repeats itself.** The line above an
  answer said "answered" twice, and claimed an answer was "answered by
  Claude" while it was still being written. It now reads once and matches
  what is actually happening: `Claude is answering`, `queued for Claude`, or
  `Claude couldn't answer`.
- **Building AMF from source on macOS is warning-free again.** Linux-only
  memory probes are no longer compiled into the macOS binary as unused code.

### Migration

No migration is required.

## [v0.35.0] - 2026-08-12

### Added

- **Learning Mode: read a codebase and ask an agent about it.** Press `K` on a
  project or feature to open a read-only viewer. Browse every file in the
  project or just the ones your branch changed, point at a whole file, a line
  range, or a single change, and ask about it. Nothing in this mode edits your
  files.
- **Two kinds of question.** `e` asks for an explanation and gets a teaching
  answer that proposes no changes; `c` asks for a concrete change you can act
  on later. `t` offers starter questions matched to whatever you have
  selected, for when you are not sure what to ask — each one lands in the
  prompt as editable text rather than being sent for you.
- **Answers are written for the reader you say you are.** `L` switches between
  newcomer answers, which define their terms and end with what to read next,
  and familiar answers, which skip the groundwork.
- **Ask again about the answer you just got.** `F` asks a follow-up, carrying
  the earlier question and answer along so "wait, what's a trait?" is answered
  against what you were just told rather than from scratch. Follow-ups stay
  attached to the code they started from, even if you have browsed elsewhere
  since, and appear indented under the answer they continue.
- **Doubt an answer and make it go check.** Most answers are written from just
  the code on screen, so they can confidently name files, symbols, or line
  numbers that do not exist. `D` asks the same question again with the whole
  repository readable, and keeps the first answer alongside the new one so you
  can compare. The rerun does not see the answer it is checking, so it works
  the question out again rather than agreeing with a guess — and it is told to
  go and read the code, and to name the files it checked, so "read the repo" on
  the row means it actually did. Following up on the verified answer continues
  from that one; the answer it replaced is left behind rather than quietly
  carried into the next question.
- **Re-file an entry once you know what it really was.** Asking picks explain
  or change up front, which is exactly the choice a newcomer is least placed to
  make — you often only learn that an explanation was really a bug report by
  reading it. `i` moves an entry to the other kind, in the history or while
  reading the answer. The answer text is kept exactly as it was and the banner
  says so, because re-filing changes how the entry is labelled, not what the
  agent said; `F` is what gets an answer written the other way. The new label
  is what the next follow-up starts from.
- **Keep an answer as something to come back to.** `a` turns any answered entry
  into an item on the project's TODO list, carrying the file and line range it
  was asked about, the question, and an excerpt of the answer — so the note
  still makes sense weeks later, and an agent spawned from it (`g` in the TODOs
  overlay) gets the whole context. Nothing is written by the keypress itself: a
  confirmation opens first, with an editable title seeded from the answer and
  the exact note it would add, and it says plainly that this writes a note about
  your code rather than a change to it. If the project has no TODO list yet, one
  is created along with the session row that makes it reachable from the
  dashboard. Entries that have been kept are marked `→ TODO`, and pressing `a`
  again opens that item instead of adding a second — unless you deleted it over
  there, in which case it says so and offers a new one.
- **Asking never blocks.** Questions are answered in the background and several
  can be in flight at once, so you can keep reading while answers arrive. Your
  questions and answers are kept per project and are still there next time you
  open the mode.
- **Pick which agent answers.** `m` chooses among the harnesses you have
  enabled. Each question runs that agent's CLI, so questions cost whatever that
  agent costs.

Learning Mode opens its own help with `?`, and shows it once automatically the
first time you open a project. No migration is required; the mode adds its own
storage the first time it runs.

- **AMF now warns before it fills the machine up, instead of after.** Starting
  an agent — a feature, or a new agent session — first checks how many agent
  sessions are already running across every project (plus any headless review
  or plan-interview run in flight) and how much memory the OS says is left. If
  either is past its threshold, one dialog says which and by how much, and `y`
  starts it anyway. It is never a refusal, and terminals, editors, and TODOs
  sessions are neither counted nor gated. Two new settings drive it:
  `max_concurrent_agents` (default `4`) and `low_memory_warn_mb` (default
  `1536`); set either to `0` to turn that half off. The defaults are sized from
  measured idle memory per harness — the README's Configuration section has
  the numbers and the reasoning. Platforms where AMF cannot read a trustworthy
  memory figure simply run on the agent count alone.

- **Stopping a feature now closes the editor AMF opened for it.** Editors were
  the one thing a feature owned that lived outside tmux, so stopping it left
  the window — and the language server underneath, routinely the single
  largest process on the box — running until you noticed. AMF now opens VS Code
  with `--new-window` and remembers which window it created, and stopping (or
  deleting) the feature ends that window and its children. It only ever closes
  a window it can prove it opened and can still identify; anything else is
  reported as left alone, never guessed at. Because VS Code runs one
  application process for the whole machine, a window that has since become
  home to other windows you opened is also left alone — reclaiming memory is
  never worth closing your other editors. Stopping a feature in the seconds
  before its window finishes opening is handled too: the window is closed as
  soon as it appears, and the stop says so. Set `kill_editor_on_stop` to
  `false` to keep the old behavior. Note that under WSL, `code` hands off to
  the Windows side and no local window process exists for AMF to own, so it
  will report a skip there.

- **`z` on the dashboard lists dormant features.** A feature counts as dormant
  only when both halves hold: its agent has produced no output for
  `dormant_idle_minutes` (default 60) *and* you have not opened it for
  `dormant_last_accessed_hours` (default 4). Either signal alone is misleading
  — a quiet agent may be waiting on you, and a feature you have not clicked
  into may be mid-run. Each row shows how long it has been quiet, how long
  since you looked at it, and whether a tracked editor is still alive, with
  `x` stop, `e` close just the editor, `d` delete, and `Enter` to jump in.

- **`amf doctor` reports what AMF is putting on this machine.** Agent sessions
  against your limit, editor windows open alongside them, memory and swap,
  `amf-*` tmux sessions with no matching feature, worktrees on disk with no
  matching feature, and editors still running for features you stopped.
  Editors sit next to the agent count rather than inside it — one language
  server can outweigh five harnesses, so a single number could not price both
  — and when the pre-start warning fires on memory it names the open editors
  for the same reason. `--json` emits the same findings for
  scripting. It is advice only — it stops nothing, kills nothing, deletes
  nothing, and always exits `0`. That extends to its own files: it opens the
  database strictly read-only, so unlike launching AMF it will not create it,
  migrate its schema, or rewrite a byte of it, and it will not create a
  `config.json` on a machine that has none. On a machine AMF has never run on
  it reports on the machine alone and says so. Under WSL it points at where swap is
  configured while being explicit that adding swap trades an out-of-memory kill
  for heavy paging, and that lowering `max_concurrent_agents` is the fix for
  the cause.
- **AI-drafted PR replies disclose how they were generated.** An unchanged
  reply returned by a PR Triage fix session now includes the agent harness,
  best-effort model name, estimated token usage, and estimated cost for that
  fix before AMF's existing AI-attribution footer. The details are recorded
  against the session the fix was injected into, so re-opening PR Triage,
  switching fix targets, or removing a session never re-attributes a draft to a
  different agent — a draft whose session is gone keeps its harness and model
  and reports usage as unavailable. Harnesses that do not expose model or usage
  telemetry say that it is unreported or unavailable instead of silently
  omitting the field. Editing the draft still changes it to a user-authored,
  AMF-posted reply and removes the AI generation disclosure. The database
  migrates in place on first launch; drafts captured before the upgrade post
  with their details marked unreported.

- **Final Review has a `?` key that shows you all of its keys.** The review
  screen has grown a lot of bindings, and the two footer rows can only ever
  show the ones that apply right now — so keys you had not used yet were
  effectively invisible. Pressing `?` (at the top level or with the line cursor
  active) opens a scrollable list of the whole key surface, grouped by what you
  are trying to do: Verdicts, Comments, Line cursor, Moving around, Reading the
  diff, Context and AI passes, and Finishing. The actions that spend tokens
  (`w` walkthrough, `A` AI co-review, `O` changeset overview) are labelled as
  such, and `I` is marked as the local, free one. `j`/`k`, PageUp/PageDown and
  `g`/`G` scroll it; `?`, `q` or `Esc` closes it. While it is open it takes
  every key, so the key you press to dismiss it cannot also approve a file or
  start the finish flow. The footer now carries a `? keys` hint pointing at it.
  The plain diff viewer is unchanged — its own footer still covers everything
  it does.

- **Pi can now power its own plan interviews.** When the installed Pi CLI
  advertises its current safe-headless contract, AMF prefers it for Pi
  features instead of immediately falling back to Claude, Codex, or Opencode.
  Question generation and synthesis run without tools or repository-provided
  resources; directed feedback and isolated investigations receive only Pi's
  read, grep, find, and list tools. Every call is ephemeral. Older Pi versions
  keep the existing fallback behavior and do not run with weakened isolation.
  No migration is required.

- **AI Review can pick a model when it runs on Pi.** Pi previously skipped
  straight to its default model because AMF didn't pass `--model` to it;
  now that it does, the model picker opens for Pi like it does for every
  other harness, and a configured `review_model` applies to Pi runs.

- **You can open the file you are reviewing in your own editor, at the line you
  are looking at.** Sometimes you need to poke around the real file before you
  can write the comment. In Final Review, `E` suspends AMF and opens the current
  file in `$VISUAL` / `$EDITOR` (falling back to `vi`), with the cursor already
  on the line the review cursor is on; with the line cursor off it opens at the
  first hunk. Quitting the editor drops you back exactly where you were. If you
  changed the file while you were in there, AMF notices and reloads the diff, so
  your comments never end up sitting on hunks that have moved. The line jump
  works with the vi family, nano, emacs, kak, micro, helix, Sublime, Zed, and
  VS Code and its forks (which are told to wait rather than returning
  immediately); an editor AMF does not recognise simply opens at the top of the
  file rather than being handed a flag it would treat as a filename. An
  `$EDITOR` that already carries its own flags, like `emacsclient -nw`, is
  respected. Files with nothing to open — a deletion, or a binary — say so
  instead of doing nothing, and the footer hint hides itself for them. No
  configuration and no migration are required.

- **The theme picker groups subtypes instead of listing all 28 themes flat.**
  Catppuccin and Gruvbox Material alone made up 22 of the picker's 28 rows,
  burying the six standalone themes (Default, AMF, Dracula, Nord, Gruvbox
  Dark, Gruvbox Light) in the middle of a long list. Both families now
  collapse into a single row — `Catppuccin (4)`, `Gruvbox Material (18)` —
  giving eight rows at the top level. Opening a group (`Enter`) drills into
  its own screen listing just that family's variants, with the repeated
  family name dropped from each row (`Dark Hard` instead of `Gruvbox Material
  Dark Hard`); `Esc` backs out to the top level without closing the picker.
  Highlighting a group previews its first variant, matching how hovering a
  single theme already worked. Reopening the picker while a grouped theme is
  active goes straight to that group's screen with the active variant already
  selected, rather than starting over at the top. No migration is required;
  keybindings are unchanged (`T` opens the picker, `j`/`k` navigate, `Enter`
  applies or opens a group, `t` toggles transparency, `Esc`/`q` backs out or
  closes).

- **You can jump between your review comments across every file, and undo a
  verdict you did not mean to give.** In Final Review, `}` and `{` move to the
  next and previous comment anywhere in the changeset, wrapping at either end —
  so you can sweep everything you annotated before finishing without hunting
  down each file again. (`Tab` still cycles the AI's draft comments within the
  current file.) Pressing `}` with the line cursor off turns it on and starts
  with the file already on screen; a comment whose line has since moved and
  cannot be located is skipped and reported rather than jumped to blindly.
  Separately, `U` takes back the last approve, skip or rejection and puts you
  back on that file, since all three advance to the next one — an accidental
  `a` no longer means finding the file again by hand. Press it repeatedly to
  walk back through several verdicts. Undo restores the verdict exactly,
  including whether a rejection had been implied by your line comments rather
  than typed out, and touches nothing else: comments, suggestions and general
  feedback are left alone. Both hints appear in the footer only once they would
  do something. Undo covers the current sitting, so pausing and resuming a
  review starts it fresh — your verdicts themselves are still saved as before.
  No migration is required.

- **You can expand the context around a diff's hunks.** Three lines either side
  of a change often hides what you need to judge it — the enclosing function
  signature, the surrounding match arm. In Final Review and the plain diff
  viewer, `+` widens the context a step at a time (3 → 10 → 25 → 50 → the whole
  file) and `-` narrows it back; `*` jumps straight between the whole file and
  the default. The footer shows the current level as `context:10` /
  `context:file`. The level is remembered per file, so moving between files
  keeps each one where you left it, and a refresh or base-ref change re-applies
  it instead of silently collapsing your view. Comments, the line cursor and any
  range selection stay on exactly the lines they were on: narrowing the context
  refuses outright while it would hide part of a range you have selected, and
  tells you where the cursor went if its own line is no longer shown. Files that have
  nothing more to show — added, deleted, binary — say so rather than doing
  nothing. Line comments left on newly revealed context are still written to the
  feedback file and sent to the fixing agent, but are not posted inline to a PR,
  since GitHub only accepts inline comments on lines inside the PR's own diff.

- **Changed lines now highlight which words actually changed.** A modified line
  and its replacement are compared token by token, and only the parts that
  differ get a brighter background — so a one-character edit inside a long line
  no longer looks like a full rewrite. It works in both the unified and
  side-by-side layouts, and sits underneath the existing syntax highlighting
  rather than replacing it. When two lines are unrelated enough to be a
  wholesale rewrite, nothing is highlighted: the row's add/remove colour already
  says everything, and marking every token would be noise. Alongside it, `W`
  toggles `git diff -w`, hiding changes that are only whitespace; the footer
  shows `ws: shown` / `ws: ignored`.

- **The diff viewers' changed-file list is now a collapsible directory tree.**
  Files are grouped under their directories instead of repeating the full path
  on every row, so a changeset spanning many folders is far easier to scan.
  This applies both to Final Review and to the plain diff viewer (leader `d`).
  In the file list, `j`/`k` move through the tree (directory headers included),
  `z` or `Enter` folds the directory under the cursor, `Z` folds or unfolds
  everything, and `h`/`l` step out to a parent or into a directory. Parking on
  a directory never changes which file the diff shows. A folded directory
  reports what it is hiding — how many files, and during a review how many are
  still undecided, any rejections, and whether anything changed since the last
  round — so folding cannot bury outstanding work. Filters, counts and `n`/`p` file
  navigation are unaffected by folding: landing on a file inside a folded
  directory simply opens it up. No migration is required.

- **PR Triage can run a PR's fixes in a new feature of its own.** The
  fix-target picker (the prompt on the first `f`/`B` of a visit) has a third
  option: `New feature…`. It creates an isolated, worktree-backed feature just
  for that PR's triage, with its harness and vibe mode picked in one compact
  form — or from a feature preset — independently of the feature the PR was
  built in. So you can build a PR in SuperVibe and apply the review fixes under
  Vibeless supervision, and the triage agent's hooks and permissions land in its
  own worktree instead of the source feature's. The companion is seeded from the
  PR head onto its own branch (`<pr-branch>-triage`, since git can't check the
  PR's branch out twice) and is reused for every fix in that PR, across restarts
  — the pane header and the fix confirm both name it, so you always know which
  feature and mode a fix will run in. `P` and `Ctrl+Space P` follow it.
  Because that work happens off the PR branch, a new `I` key lands it: push the
  companion branch onto the PR branch, or cherry-pick into the source worktree.
  Both are explicit and show the commits first — the push is never forced, and
  the cherry-pick refuses to run while the source worktree has uncommitted
  changes. The existing two targets are unchanged and still commit straight onto
  the PR branch, so they never see the `I` step. The database gains a column for
  the PR link; it is added automatically and needs no migration step.
- **Plan-mode interviews now pause at a review gate before launching the
  feature.** An opted-in AI flow turns the interview into a structured
  implementation plan, then shows the rendered markdown for review. You can
  edit the raw plan, regenerate it, scroll through the preview, or abort;
  AMF writes `.claude/plan.md` and starts the feature only after you press
  `Enter` to accept. If synthesis is unavailable or returns an invalid plan,
  AMF shows the raw interview plan as a fallback. No migration is required.
- **The dashboard now shows when an AI Review is generating.** A feature's PR
  badge reads `[PR #123 · 4 open · AI review]` while its review runs, so you
  can leave the AI Review pane, work elsewhere, and still see the pass is in
  flight. The marker appears only on the feature whose review is running and
  clears when generation succeeds, fails, or is cancelled. No migration is
  required.
- **Final Review now includes a review-round history browser.** Press `H` to
  move between the live review and earlier rounds, including their verdict
  counts, checks, comments, suggestions, and agent replies. Older archived
  rounds load only when you navigate past the recent history, while `Enter`
  on `Current` returns directly to the editable review. No migration is
  required.
- **Plan-mode interviews now ask AI-generated follow-up questions.** After
  the configured questions, AMF offers an explicit opt-in before using any
  agent tokens. If accepted, the feature's agent harness (or an available
  fallback) tailors up to two more rounds to the brief and prior answers,
  with a progress screen while each round runs. Declining or finishing early
  uses no adaptive-interview tokens; an unavailable or failed harness lets
  the interview complete normally. No setup or migration is required.
- **Final Review shows a summary before it writes and dispatches anything.**
  `q` used to gate on undecided files and then finish immediately; it now
  opens a navigable summary listing every file's verdict, every open line/file
  comment (and suggestion), and the general feedback, in one place. `j`/`k`
  (and `g`/`G`) move through the list, `Enter` jumps back into the diff at
  that item and opens its editor pre-filled — a rejection's feedback, a line
  or file comment, or the general note — so fixing something you spot in the
  summary no longer means hunting it down again. `q` from the summary is the
  real finish; `Esc` just closes it and returns to reviewing, with nothing
  written, posted, or dispatched. No migration is required.
- **PR Triage's detail pane now shows a comment's replies, however they got
  posted.** A reply posted outside AMF's own `R`/`n` flow — e.g. an agent
  working the PR with its own `gh` access — previously left no trace next to
  the original comment; you had to hunt the flat list for the reply entry. A
  new "Replies" section lists each reply with its author, a `[via AMF]` chip
  when it carries AMF's own posting disclosure, and the thread's current
  `[outdated]`/`[✓ resolved]` chips, so confirming a thread already got
  answered is a glance at the comment, refreshed on demand with `r`. The
  `pr-continue` skill also now explicitly avoids posting a "done" reply on
  its own initiative, and only cites a commit after confirming it actually
  touches the comment's file.
- **Review actions can each use a different model.** A new `review_models`
  config setting maps an action name (`walkthrough`, `co_review`,
  `changeset_overview`, `diff_explain`, `pr_review`, `review_memory`) to its
  own `--model` override, so a whole-changeset overview can run on a
  stronger model while a single-file walkthrough runs on a cheaper one. Any
  action left unset still falls back to the existing shared `review_model`
  setting, so nothing changes for configs that don't opt in. No migration is
  required; existing `review_model` configs continue to work.
- **Review Mode note reads stay bounded without losing history.** AMF now
  keeps only the latest note for each of the 50 most recently documented
  files in `.claude/review-notes.md`, moving older and superseded sections
  to `.claude/review-notes-archive.md` after each agent turn. Review
  surfaces merge both files, while agents only re-read the small live file.
  No migration is required; AMF archives existing notes automatically.
- **Codex can now capture visual proof of AMF UI changes.** Codex features
  receive an `amf-screenshot` skill that drives the same isolated screenshot
  harness as Claude, verifies captured frames, and returns viewable PNG or GIF
  files without touching the user's real AMF database or tmux sessions.
- **Leaving a plan interview no longer loses your answers.** Answers are saved
  as you give them, so aborting the interview, cancelling the feature, or
  closing AMF entirely keeps them. Starting the interview again for the same
  feature offers to resume the saved draft or discard it and start over, and
  tells you when it was saved, how much was answered, and whether a plan had
  already been generated — nothing is restored until you choose.
- **Resuming picks up where you stopped.** You land on the first question still
  unanswered rather than walking forward through answers you already gave.
  Adaptive AI rounds you already paid for are kept, and a draft that already
  had a generated plan reopens at the review gate instead of generating a
  second one.
- **Accepting a plan keeps the interview behind it.** The questions and answers
  are stored with the feature and used to pre-fill a re-run. Deleting a feature
  removes its stored interviews.
- **Re-planning a feature starts from the plan you already accepted.** Running
  the interview again on a feature fills in the brief and every answer from the
  last plan you accepted for it: `Enter` keeps an answer, typing changes it, and
  `Ctrl+R` restores it if you change your mind. Each question says whether its
  answer is still the previous one, has been changed, or has been cleared —
  clearing one drops it from the new plan. If you edited a multiple-choice
  question's options in `plan_questions` since then, its old answer is left out
  rather than pre-filled as a choice that no longer exists. Follow-up questions
  an AI round asked last time are asked again with their answers, but adaptive
  rounds are not reused: a re-run asks for its own opt-in before spending any
  tokens.
- **You can now plan a feature you already created.** Press `P` on a feature, or
  pick `plan-interview` from the command picker, to run the same interview
  without going through the creation wizard. Accepting rewrites that feature's
  own plan, turns plan mode on for it, and points its agent at the plan —
  previously the interview only ran while creating a feature. Leaving the
  interview is non-destructive: the feature keeps whatever plan it had.
- **A running session can now be told about a plan you just accepted.** An
  agent reads its instruction file once, at startup, so re-planning a feature
  whose session was already running used to leave that session working from the
  old plan. Accepting now offers to open the running session with a kickoff
  prompt pointing at the new plan — seeded in the composer, not sent, so you
  decide when it lands. Declining costs nothing: the plan is written either way.
- **Plan drafts now accept direct feedback before approval.** Press `f` at the
  review gate to tell the planning agent exactly what to change. The agent can
  inspect the feature repository read-only when the request needs concrete
  code locations, then returns a revised draft for you to review before
  accepting it. Failed revisions keep both the current plan and your
  instruction so you can retry without retyping it.
- **Plan research can now stay out of the implementation session's context.**
  Press `i` at the plan review gate and enter up to four research focuses,
  separated by blank lines. AMF runs each in a fresh read-only agent context,
  validates and bounds the findings, then gives only those findings to a
  separate no-tools planning pass that merges them into the draft. Failed or
  dismissed investigations leave the current plan untouched, and a failure
  preserves the research request for retry. No migration is required.

### Fixed

- **Vibeless review stays with the feature that enabled it.** A Vibeless
  feature using the main repository no longer causes inherited diff-review
  popups in later worktree features. AMF scopes existing hooks automatically
  on startup; no manual configuration changes are required.

### Migration

No manual migration is required. AMF creates and migrates the new local
storage automatically and refreshes managed feature hooks on startup.

## [v0.34.1] - 2026-08-12

### Fixed

- **Older macOS Claude hooks are repaired before they can run.** AMF now checks
  worktree-local settings, root-repository settings inherited by worktrees,
  legacy project settings, and Claude's global settings. Old unquoted commands
  under `~/Library/Application Support/amf` no longer keep producing
  `PostToolUse: Bash hook error`, including when Claude is a secondary session
  in a Codex, OpenCode, or Pi feature. Generated Claude scripts now live under
  the space-free `~/.amf/hooks` path and use Claude's direct command form.

### Migration

No manual settings edits are required. After upgrading, restart AMF and restart
affected features so Claude reloads the automatically repaired hook settings.

## [v0.34.0] - 2026-08-07

### Added

- **Final Review now has a complete key reference.** Press `?` from the review
  or line cursor to open a scrollable, grouped list of every review shortcut,
  including clear labels for actions that spend agent tokens.
- **Final Review can open the current file in your editor.** Press `E` to open
  the reviewed file at the cursored line using `$VISUAL` or `$EDITOR`.
  Returning to AMF reloads the diff if the file changed.
- **Final Review can navigate comments across files and undo verdicts.** Use
  `{` and `}` to visit review comments throughout the changeset, and `U`
  to restore the most recent approve, skip, or rejection.
- **Diff review has richer context controls.** Use `+`, `-`, or `*` to
  expand and contract hunks up to the whole file, and `W` to hide
  whitespace-only changes. Modified lines also highlight the exact words that
  changed in unified and side-by-side layouts.
- **Plan interviews can be resumed, rerun, and used on existing features.**
  Answers and generated drafts survive cancellation or restart, accepted
  answers prefill later planning passes, and `P` starts an interview for a
  feature that already exists.
- **Plans can be refined without filling the implementation session with
  research.** Press `f` to give a draft direct revision feedback or `i` to
  run isolated, read-only investigations. After accepting a new plan for a
  running feature, AMF can seed a handoff prompt into that feature's composer.
- **Pi can power plan interviews and expose model selection for AI Review.**
  Current Pi installations use the safe headless flow; older versions keep the
  existing fallback to another available harness.
- **The theme picker groups large theme families.** Catppuccin and Gruvbox
  Material now open into their own variant screens, keeping the top-level
  picker short while preserving live previews and existing shortcuts.
- **Agents working on AMF can capture UI proof.** This repository includes
  native Claude and Codex `amf-screenshot` skills for isolated PNG and GIF
  capture without touching the user's real AMF database or tmux sessions.
### Changed

- **The AMF screenshot skill stays with AMF.** Feature setup no longer injects
  this repository-specific development workflow into unrelated projects.
- **Global review memory can be compacted from PR Triage.** Press `c`, then
  `g`, to select the cross-project memory document. AMF preserves findings
  added by another session while compaction is running and asks before any
  conflicting overwrite.
- **Session summaries use the feature's own harness.** Codex, OpenCode, and Pi
  features no longer launch Claude just to generate their one-line summary.
- **Plan interviews are clearer and safer at their edges.** The optional AI
  step identifies token-using choices, the default interview is shorter,
  presets enter the same guided flow, projects with no questions can draft
  from the brief, and oversized answers are bounded only when sent to an AI.

### Fixed

- **Pane feedback no longer covers agent output indefinitely.** Repaint and
  status confirmations appear as timed toasts and clear automatically.
- **Custom-session icons can be chosen visually.** The config wizard previews
  useful Nerd Font icons and correctly renders existing bundled icon names.
- **Managed tmux hyperlinks work on macOS.** OSC 8 links from agent sessions
  open normally after the session is reopened.
- **Final Review no longer clips its footer shortcuts.** Both hint rows receive
  their own space, including at narrower terminal widths, while short terminals
  still preserve room for the diff.

### Migration

No migration is required. AMF creates plan-interview storage automatically.
Existing saved sessions pick up the macOS hyperlink fix when reopened, and
Codex features receive the screenshot skill when their local setup next runs.

## [v0.33.0] - 2026-07-28

### Added

- **Plan-mode interviews now stop at a review gate before launching.** Review
  the rendered plan, edit its source, regenerate it, or abort. AMF writes the
  plan and starts the feature only after you explicitly accept it.
- **Plan interviews can ask optional AI-generated follow-up questions.** After
  the configured questions, you can spend agent tokens on up to two adaptive
  rounds tailored to the brief and your answers. Declining or finishing early
  uses no adaptive-interview tokens.
- **Plans can receive an optional agent review before acceptance.** Press
  `a` at the review gate to check the draft for gaps, risks, contradictions,
  and missing acceptance criteria. The review never edits the plan directly;
  press `r` to generate a revision from its feedback. Completed reviews are
  retained if you leave and return.
- **Diff file lists are now collapsible directory trees.** Final Review and
  the plain diff viewer group files by folder. Use `z` or `Enter` to fold
  one directory, `Z` to fold all, and `h`/`l` to move through the tree.
  Folded directories summarize hidden files and outstanding review work.
- **Review memory now has a cross-project layer.** AI Review reads both the
  repository memory and a personal memory file at
  `~/.config/amf/review-memory.md`. Use `g` when adding or bootstrapping
  memory to choose the global destination; project memory remains the
  default.
- **PR Triage fixes can run in a dedicated companion feature.** Choose
  `New feature…` as the fix target to create an isolated worktree with its
  own harness, mode, or preset. AMF reuses that companion for the PR and
  clearly identifies where fixes are running. Press `I` to review and land
  its commits by pushing to the PR branch or cherry-picking into the source
  worktree.
- **The dashboard shows active AI Review generation.** The relevant feature's
  PR badge includes `AI review` while generation is running and clears when
  it finishes, fails, or is cancelled.
- **Final Review includes a review-round history browser.** Press `H` to
  inspect earlier verdicts, checks, comments, suggestions, and agent replies,
  then return directly to the editable current review.
- **Review actions can use different models.** The new `review_models`
  setting supports per-action overrides for walkthroughs, co-review,
  changeset overviews, explanations, PR review, and review memory. Unset
  actions continue to use `review_model`.
- **Review Mode keeps note reads bounded without losing history.** The live
  notes file retains the latest note for each of the 50 most recently
  documented files; older and superseded notes move to an archive that review
  surfaces still read.

### Changed

- **Final Review leaves more room for the diff.** The Developer Notes panel is
  now about half its previous height. Press `e` to expand it when needed.

### Fixed

- **Agent sessions can recover after their tmux process disappears.** Opening
  a saved Claude, Codex, or OpenCode session whose tmux session was lost now
  offers to resume it, start clean, choose another saved session, or cancel.
- **AI Review's model picker can return to harness selection.** Press
  `Esc` or `q` to correct the harness without leaving AI Review; model
  choices are rebuilt for the newly selected harness.
- **AMF-authored follow-up replies no longer crowd out new PR feedback.**
  Replies stay attached to their original comment after refresh, while
  standalone findings remain actionable.

### Migration

- No migration is required. AMF adds the PR companion-feature database field
  and archives older Review Mode notes automatically. Existing
  `review_model` settings continue to work unchanged.

## [v0.32.0] - 2026-07-24

### Added

- **Diff view can isolate a single commit.** Leader `d` opens a scope picker
  where you can keep the full current changeset or choose one feature-branch
  commit and review only what it introduced.
- **Final Review has a confirmation summary.** Pressing `q` now opens a
  navigable list of every verdict, unresolved comment or suggestion, and
  general feedback. Jump back to edit any item with `Enter`, press `Esc` to
  keep reviewing without side effects, or press `q` again to finish.
- **PR fixes prepare editable reply drafts.** After an agent addresses a
  comment sent with `f` or `B`, pressing `R` prefills its reviewer-facing
  response. AMF labels the reply as AI-drafted, chooses a relevant fix commit
  when possible, and still requires confirmation before posting.
- **Completed AI Reviews stay pending in PR Triage.** A persistent badge shows
  how many findings remain publishable, and `A` reopens the cached review
  instead of running it again. Reviews also include an editable overall
  summary, and PR Triage refreshes automatically after posting.
- **PR Triage shows replies with their original comment.** The detail pane
  lists replies even when they were posted outside AMF, including their
  author, AMF attribution, and current outdated or resolved state.

### Migration

- No migration is required. Older AI Review drafts without an overall summary
  continue to use the existing fallback text.

## [v0.31.0] - 2026-07-20

### Added

- **AI Review now has a dedicated pane, separate from PR Triage.** Open it
  from PR Triage with `A`, from the dashboard or PR picker with `W`, or
  from a live agent session with leader `W`. Review findings, edit or skip
  them, and post the kept findings as one GitHub review. Long-running reviews
  now show live activity, elapsed time, and token usage for Claude, Codex,
  OpenCode, and Pi.
- **Review memory can now be compacted from the PR picker.** Press `c` to
  merge duplicate guidance and remove stale or overly specific findings.
  AMF shows the proposed rewrite in an editable full-screen preview and does
  not write it until you confirm.
- **Final Review supports file-level comments.** Press `m` on a file to
  leave an observation, question, nit, or praise without rejecting it, and
  `M` to resolve or reopen the thread. File comments persist across review
  rounds and can be posted to GitHub.
- **Final Review can apply suggested changes.** Press `x` on a suggestion
  to apply it immediately, or `X` to apply all remaining suggestions when
  the review finishes. AMF skips files that changed after the diff loaded and
  reports anything that still needs attention.
- **Final Review can be paused and resumed.** Top-level `Esc` now returns to
  the feature without writing feedback, posting to GitHub, or dispatching a
  fix; reopening Final Review restores the same decisions, comments, and
  filters. Press `q` to finish as before.
- **Review memory paths can be configured per project.** Set
  `review_memory_path` in `.amf/config.json` to override the global path
  for that repository.
- **AI Review has an in-pane model picker.** Choose the harness default, a
  verified Claude preset, or a custom model before starting a review. The
  existing `review_model` setting provides the initial selection.
- **PR Triage is available inside live agent sessions.** Leader `G` opens
  the current feature's PR, while an ambient badge or sidebar panel shows the
  PR number, open-comment count, and whether triage or AI review work is
  running.
- **PR conversations can be grouped separately.** Press `o` to cycle to
  “conversations last,” which places top-level discussion after code comments
  under a visible divider.

### Changed

- **PR Triage uses fewer top-level shortcuts.** Fix targets are chosen when
  you first press `f` or `B`; `R` opens the reply-template picker; and
  `m` opens the local/GitHub mark-action picker. All previous workflows
  remain available through these focused pickers.
- **Review Mode uses fewer tokens.** Review notes are batched per touched
  file, bounded Final Review passes honor `review_model`, and only the two
  newest feedback rounds stay in the live feedback file. Older rounds move
  to a gitignored archive.
- **New-session setup now labels Review Mode as high token usage.** Existing
  sessions and defaults are unchanged.
- **Templated PR replies now disclose that AMF posted them.** “Done” and
  “Not needed” replies receive a lightweight attribution footer that is
  previewed before posting.
- **“Done” replies choose a more relevant commit.** AMF checks the commented
  line and file history before falling back to the current `HEAD`.
- **Final Review's layout status is clearer.** The footer shows the layout
  actually in use, and AMF explains why new or untracked files cannot switch
  away from unified view.

### Fixed

- **Stale Claude hook entries are cleaned up with custom config roots.** AMF
  now recognizes its managed hooks under any `amf` config directory and
  automatically replaces obsolete paths the next time feature hooks refresh.
- **File-level comment editing is visible in Final Review.** The editor now
  receives the same expanded space as line comments and rejection feedback.
- **Long theme lists keep the selection visible.** The picker scrolls as you
  navigate and shows the current position.
- **The ambient PR indicator remains visible and current while composing.**
  It now refreshes independently while you stay inside an agent session.
- **PR fixes warn before targeting the wrong branch.** When the loaded PR
  branch differs from the worktree's checked-out branch, AMF shows the
  mismatch both in the pane and before fix injection.
- **Composer paste works on macOS.** Both text and image clipboard content can
  now be pasted through AMF.

### Migration

- No migration is required. AMF upgrades its local database and cleans stale
  managed hooks automatically. Existing AI Review drafts from PR Triage are
  not moved into the new pane; regenerate any draft you still need with `A`.

## [v0.30.1] - 2026-07-14

### Fixed

- **Builds work again on musl Linux and macOS.** A platform-specific type
  mismatch in AMF's terminal-attach code broke `cargo build` — and the
  downloadable release binaries built from it — on those targets. No
  migration is required.

## [v0.30.0] - 2026-07-14

### Removed

- Removed the PR Triage `F` shortcut that queued marked fixes immediately.
  Use `B` to review and edit one combined prompt before sending the batch.

### Added

- **PR Triage surfaces the last AI review's outcome, not just while it's
  running.** Previously, pressing `A` and then leaving the pane (or closing
  and reopening AMF) left no trace of what happened — the findings were
  cached, but nothing showed whether a review had even run. The pane header
  now shows a badge for the most recent `A` run on the current head SHA: "AI
  review: N findings (5m)", "AI review: no findings (2h)", or "AI review
  failed (1h): `<error>`" — colored as a warning for a failure. The badge
  persists across a same-head-SHA cache-hit reopen and survives an AMF
  restart; it clears automatically once the PR's head SHA moves (a push
  means the record no longer describes the current diff). No setup or
  migration required.
- **File-level PR comments for whole-file rejections.** When posting a
  finished final review to its GitHub PR, a file you rejected without
  anchoring feedback to a specific line now posts as its own comment
  attached to that file — instead of being dumped as a paragraph in the
  review's summary body alongside every other whole-file rejection.
- **"Fixes ready — re-review?" notification.** After you finish a review
  and it dispatches feedback to an agent, AMF now watches that session and
  notifies you once the agent has finished working through it — instead of
  you polling the pane to see if it's done. Selecting the notification jumps
  straight back into the review, pre-filtered to just the files that
  changed since your last pass.
- **PR Triage shows whether its dedicated fix session is working.** Once the
  session exists, the pane header shows `[dedicated ● working]` while that
  exact agent session is thinking or running a tool, and `[dedicated idle]`
  when it is waiting or finished. This now follows the dedicated session
  correctly whether it uses Claude, Codex, OpenCode, or Pi—even when the
  feature's primary session uses a different harness. Activity in another
  agent window no longer affects this status. No setup or migration is
  required.
- A design doc for the planned **plan-mode guided discovery interview**
  in `docs/backlog/plan-mode-interview-plan.md`, viewable in AMF's
  in-app Markdown viewer alongside the other plans. It captures where
  plan mode is headed — AMF interviews you about a feature (curated and
  per-project questions plus AI follow-ups from your feature's own
  agent harness), synthesizes a structured plan you review and edit,
  and only then launches the agent seeded with it — including the
  decision to replace the current shared repo-root `PLAN.md` with a
  per-feature `.claude/plan.md`. Plan-mode feature creation now opens a
  native guided interview before the feature or agent launches. The flow
  collects a required brief and curated follow-up answers, supports
  multi-line input, back navigation, optional-question skipping, and
  finishing early, then continues the deferred launch. Cancelling offers a
  clear choice to resume, launch without a plan, or cancel feature creation
  while preserving any worktree that was already created. No setup or
  migration is required. Completing the interview now saves the brief and
  answers to the feature's gitignored `.claude/plan.md` before launch, so the
  resulting plan stays isolated to that feature and is ready for the agent and
  AMF's plan preview. This also applies to the first feature that runs directly
  in the project repository: the agent and sidebar use the same
  `.claude/plan.md`, without creating a second root-level plan. Plan
  instructions now follow the feature's selected harness: Claude reads them
  from its local instruction file, while Codex, OpenCode, and Pi receive them
  through `AGENTS.md`. AMF removes its managed instructions when the feature
  stops or is deleted and moves them when the harness changes. It no longer
  creates or updates the old shared repo-root `PLAN.md`; existing copies are
  left untouched and can be deleted if they are no longer needed. The `?`
  help overlay and README now document every interview shortcut, including
  multi-line answers, back navigation, skipping, finishing early, and cancel.
- **Plan-mode interviews support custom questions.** Add `plan_questions` to
  global or project AMF config to append free-text questions, offer selectable
  answers with an `options` list, or replace a built-in question by reusing its
  ID. Project questions override global questions with the same ID, and
  `skip_builtin_questions` can run an interview using only configured
  questions. The config wizard now manages these questions at either scope:
  add free-text or select questions, mark them optional, and toggle the built-in
  question bank without editing JSON by hand. IDs are normalized before global
  and project questions are merged, so surrounding whitespace cannot prevent a
  project override. The interview also labels global and project templates with
  their actual scope. Existing configs keep the built-in interview unchanged,
  so no migration is required.
- **AI follow-up groundwork for plan-mode interviews.** AMF now prepares
  harness-neutral follow-up requests from the feature brief, prior answers, and
  a bounded snapshot of the repository's README, top-level layout, and project
  instructions. Adaptive questions are not shown in the interview UI yet; this
  prepares the next plan-mode milestone. No setup or migration is required.
- **Your own PRs are highlighted in the PR picker.** The picker (`G` with no
  branch PR, or `g` inside the review pane) now bolds your `@login` and tags
  it `you` when a row is one of your own PRs, so you don't have to read every
  author to find your work in a shared repo's PR list.
- **Open PRs are visible directly on the dashboard.** Feature rows now show the
  PR number and unresolved review-thread count (for example,
  `[PR #321 · 4 open]`), so you can see which branches need triage before
  opening the PR pane. The badge refreshes in the background and keeps its
  last known value through temporary GitHub errors. No setup or migration is
  required.
- **PR triage shows what the current visit costs.** After a fix uses the
  dedicated or existing-live agent session, the PR-review header now shows the
  tokens and estimated cost added during this visit alongside the session's
  lifetime usage. Earlier work in a reused session is excluded, and the tally
  survives refreshing comments or jumping into the fix session and back. No
  setup or migration is required.
- **AI-authored PR review comments are attributed.** When you post an AI
  review (`W` in the PR-review pane), each inline comment now carries a
  small `— drafted by AI via AMF` footer, so a reviewer looking at just
  that comment — without the review summary in view — can still tell it's
  AI-authored rather than mistake it for your own words. Your own typed
  replies (the "not needed" explanation, a "done in `<sha>`" note) are
  never touched.
- **Choose the harness that generates an AI PR review.** The first time you
  press `A` in a PR Triage pane, AMF now offers the project's enabled Claude,
  Codex, OpenCode, and Pi harnesses, defaults to the project's preference, and
  remembers the choice for later reviews in that pane. This choice is separate
  from the harness used for `f`/`B` fixes, so a rate limit or outage on one
  provider no longer blocks review generation when another is available. AMF
  validates the selected CLI before fetching the diff or spending tokens, and
  provider-specific failures remain visible in PR Triage. No setup or migration
  is required.
- **AI review of the PR diff (draft findings).** Press `A` in the PR-review
  pane to have AMF review the PR's diff itself and surface findings as draft
  items in the same list, triaged with the verbs already there (`f` inject-fix
  · `s` skip · `M` add to memory). The review-memory doc is injected as
  context so it checks the team's known recurring issues first; an optional
  `ai_review_skill` config setting (e.g. `"review"`) leads the prompt with an
  existing review skill/command as the primary methodology, if you
  have one installed. Draft findings persist in the PR's cache so re-opening
  the pane at the same commit replays them without spending tokens again; a
  manual refresh (`r`) carries them forward too, unless the PR has moved to a
  new commit. This is an explicit, opt-in action — the running screen shows a
  token estimate before the one paid pass, and `esc` lets it keep going in the
  background. Once vetted (skip what you disagree with), `W` posts the
  remaining findings to GitHub as a real review — anchored ones as inline
  comments, everything else folded into an editable summary — always as a
  `COMMENT` event (never auto-approve/request-changes). The running screen's
  throbber now animates properly for the (potentially long) blocking `gh`/
  agent call rather than sitting frozen, and `esc`-ing back to the pane
  while it's still running shows a throbber + "AI review running…" in the
  header, so neither screen reads as stuck or stalled. Success/warning/error
  toasts (e.g. "AI review found N findings") now actually render while any of
  the PR-review pane's full-screen modes are showing — they were being pushed
  but silently swallowed before. A failed `A` run now uses that visible error
  toast too, instead of putting the failure in a dashboard-only message that
  cannot be seen after returning to PR Triage. Finding-parsing also tolerates
  a model that doesn't hold the exact requested heading level or wraps its
  whole reply in a code fence, and a `0 findings` result is now distinguished
  (a warning toast + a debug-log dump of the raw response) from a quiet
  success, so a parsing mismatch doesn't look identical to "the diff was just
  clean". A
  draft finding's role chip now reads `[ai]` instead of falling through to
  `[human]`, its detail pane shows a small window of the actual diff around
  its line (re-matched from the PR diff by `path:line`, same as a fetched
  GitHub comment — capped to a few lines of context on each side rather than
  the whole matched hunk, which for a large added block can otherwise read
  as "the whole file") instead of nothing, and `esc`-ing back to the pane
  before the background pass
  finishes no longer silently drops the findings while still claiming
  success — they now merge into wherever you actually are when it lands.
- **"Since last review" interdiff in the final review.** Press `I` on a
  file flagged `Δ` (changed since your last review round) to see just the
  diff between what you reviewed last time and what's there now, instead of
  re-reading the whole base-ref diff to spot the fix. Computed on demand
  from a snapshot of the file's content taken when the last round finished
  — no extra config, and it's a no-op with a message on a first-ever review
  or a file whose diff moved for reasons other than its own content (e.g.
  the base ref shifted).
- **Build/test gate before finishing a final review.** Point
  `final_review_check_command` at your project's build, test, or proof
  script in `.amf/config.json` (or globally in `~/.config/amf/config.json`;
  project overrides global, same as lifecycle hooks) and AMF runs it in the
  background when you finish a review — no config means no change, it's
  entirely opt-in. The command doesn't block the UI while it runs, and
  pass/fail shows up in the finish summary and a `Check` section in
  `.claude/final-review-feedback.md`. A failing check is never silently
  swallowed by an all-approve: even with zero rejections, a failing check
  still writes and dispatches the round so the agent sees it.
- **Review-memory lookback bootstrap.** Press `b` in the PR picker to seed
  `.amf/review-memory.md` from your PR history instead of building it up one
  comment at a time. Pick a depth (20 / 50 / 100 / all recent merged & closed
  PRs); AMF fetches their review comments and summaries via `gh` (zero agent
  tokens — bot boilerplate stripped, same as everywhere else in PR review),
  then makes **one** agent pass to cluster the recurring findings and appends
  the new ones (dedup-aware, so re-running over overlapping history is a
  no-op). The running screen shows the PR count and a token estimate before
  that one pass, and `esc` returns you to the picker without losing the run —
  it keeps going in the background and still reports its result when done.
- **Review-findings memory doc groundwork (no UI yet).** Added the on-disk
  primitives for `.amf/review-memory.md` — a version-controlled file that
  will accumulate the team's recurring code-review findings, grouped by
  category, for the upcoming AI reviewer to read as context. AMF only ever
  appends to it (dedup-aware) and never rewrites existing prose; the
  location is configurable if you'd rather keep it somewhere other than
  `.amf/review-memory.md`. This is groundwork only — there's no pane key or
  agent action wired up to it yet; that lands with the PR-review lookback
  bootstrap and "add to memory" key.
- **"Add to memory" key in the PR review pane.** Press `M` on a selected
  comment to append its distilled finding to `.amf/review-memory.md` — seeded
  from the bot-stripped comment text plus a `file:line` hint, editable before
  it's saved, with `Tab` cycling a category (Concurrency, Error handling,
  Naming, Tests, …). Appends are dedup-aware, so re-adding the same finding
  is a no-op rather than a duplicate line. Zero agent tokens — a local file
  write only, and only on your explicit confirm. This is the incremental way
  the review-memory doc grows during normal triage.
- **Token usage shown in the PR review pane header.** Once a fix has spun
  up the dedicated (or existing-live) session, the header shows what
  triage has spent on it so far — e.g. `dedicated usage 12.3k eff · $0.15`
  — reusing the same usage tracking and cost formatting as the dashboard's
  per-feature token badge. The span is only shown when it fits on the
  header line, so it never clips on a narrow terminal.
- **Quick toggle between the PR review pane and the fix session.** Press `P`
  in the PR comment-review pane to jump into the session your fix went to
  (or is about to go to), then `Ctrl+Space` then `P` from that session to jump
  straight back — landing on the exact comment, scroll position, and marks
  you left, no reopening or re-fetching the PR. Previously the only way back
  was exiting to the dashboard and reopening the review from scratch, which
  lost your place. This also kicks in automatically after the ordinary `f`
  (inject fix) flow, not just the dedicated peek. A footer hint (`P session`)
  and a status badge (`Ctrl+Space P: back to review`) show up whenever the
  toggle is available.
- **Combined batch fix in the interactive PR review — "fix all of these, then
  I'll come back."** Mark a set of comments with `space`, then press `B` to
  build **one** numbered prompt covering all of them and inject it into the
  dedicated review session in a single shot. It uses one shared preamble plus
  a `Comment N:` entry per comment —
  each with its `file:line` pointer, comment text, and diff hunk, and (as with
  a single fix) no file contents — so a big set is the cheapest path in tokens
  and the agent works the whole list while you're away. It reuses the familiar
  fix dialog, so you still get the `~N tokens` preview, editing, and vim keys
  before sending. Everything included is marked `Fixing` and the marks clear,
  so the next refresh reconciles what actually got resolved. Very large batches
  raise a warning toast but still go through.
- **Leaner fix prompts in the interactive PR review.** When a bot comment
  (CodeRabbit, Copilot, etc.) re-pastes the diff inline as a quoted code
  block or blockquote, that repeated content is now stripped before it
  reaches the agent — you already get the diff hunk for free, so this cuts
  wasted tokens on bot-heavy PRs. The comment still displays in full, as
  written, in the detail pane.
- **Resolve / reopen a comment thread across final-review rounds.** With the
  line cursor active, press `R` on a line comment to mark it resolved, or
  again to reopen it. A resolved thread stays visible (and can still be
  reopened) but is left out of `.claude/final-review-feedback.md` and a
  posted PR review — settling a conversation actually stops re-sending it —
  and resolving a file's last open thread clears its comment-implied
  rejection the same way deleting the comment would. Threads that are still
  open when a round finishes carry into the next round automatically
  (tagged "(unresolved from a previous round)" in the feedback file so it's
  clear they're not new), and a new `Unresolved` step in the `F` file-filter
  cycle narrows the list to files with an open thread. Opening a re-review
  now also reports how many unresolved threads carried over.
- **On-demand changeset overview and risk markers in the final review.** Press
  `O` to run a headless Claude pass over every changed file at once (capped
  and bounded, so it never runs automatically or blows up token cost on a
  huge changeset) and read a short overview plus a "Risk factors" list in a
  scrollable modal — `O` again regenerates, `q`/`Esc` closes. The file list
  also flags each row with `[L,N,T]` markers: `L` for a large diff, `N` for
  no developer note or walkthrough yet, and `T` when the changeset has no
  test-looking file at all.
- **Search within the final-review diff.** Press `/` in the final review to
  incrementally search the current file's diff (case-insensitive). The line
  cursor jumps to the first match as you type; after `Enter`, `n` / `N` cycle
  matches (wrapping) and `Esc` clears the search. Every hit is flagged with a
  `▷` gutter marker and the footer shows the match count.
- **AMF is now licensed under the MIT License.** The repository ships a
  `LICENSE` file, so you can use, modify, and redistribute AMF under
  standard MIT terms. No migration is required.
- **TODOs keybindings in the help overlay.** The `?` help now has an "In the
  TODOs view" section documenting every key in a TODO list — navigate, add,
  edit title/notes/scratchpad, toggle done, cycle priority, reorder, delete,
  and spawn an agent — so they're discoverable without leaving the app.
- **Deleting a TODO list's host feature no longer loses your TODOs.** A
  project's TODO list lives under whichever feature you created it in. When you
  delete that host feature but the project still has other features, AMF now
  prompts you to **re-home** the list onto a surviving feature or **delete** it
  outright — `Esc` keeps the list by moving it to the first surviving feature.
  If no features remain, the now-orphaned list is cleaned up automatically.

### Changed

- **Codex-powered headless analysis is now isolated and disposable.** AI PR
  reviews and other one-shot Codex tasks run with a read-only sandbox, accept
  work outside a Git repository, and no longer leave temporary sessions in
  Codex history. AMF now checks that the installed Codex version supports the
  required headless flags and asks you to upgrade instead of failing after the
  task starts. No setup or migration is required.
- **PR Comment Review is now PR Triage.** Dashboard help, pickers, pane titles,
  status text, and documentation now use the outcome-focused name for the
  workflow where you triage, fix, reply to, and resolve pull-request feedback.
  New dedicated fix sessions are named `PR Triage`; existing `PR Review`
  sessions continue to be found and reused.
- **The dashboard session filter is temporarily disabled.** Pressing `f` on
  the dashboard no longer cycles session types, and the filter hint is hidden
  from the footer and help overlay. All sessions remain visible while the
  feature is paused. No migration is required.
- **Codex SuperVibe now skips Codex permission prompts.** Codex-backed
  SuperVibe sessions launch with full-access, no-approval permissions, so the
  mode now behaves consistently with its no-prompt warning.
- **Next / previous feature navigation is now opt-in.** The leader `n` / `p`
  shortcuts no longer jump between features by default, and they no longer show
  up in the leader menu unless you bind them. To get them back, bind the
  `next_feature` and `prev_feature` actions in your config (or via the config
  wizard) — they'll reappear in the menu using whatever keys you choose.
- **Freed-up shortcuts moved to lowercase.** With `n` / `p` no longer taken by
  feature navigation, the prompt library (leader menu) and the syntax parser
  picker (dashboard) both moved from `P` to `p`.
- **Building from source is now warning-free.** `cargo build` and
  `cargo clippy` complete without a single warning, so installing AMF from
  source no longer scrolls lint noise past you. CI now enforces this
  (`clippy -D warnings` plus a formatting check), so it stays that way.
  Nothing about AMF's behavior changes.
- **The README caught up with the app.** It now covers the Pi harness, the
  first-run harness setup wizard, the final-review workflow (including the AI
  co-reviewer and suggested changes), PR comment review, per-project TODO
  lists, and the usage/cost meters — and the keybinding tables match the
  in-app help overlay again. Config examples were corrected too: agent and
  mode values in `config.json` must be lowercase (`"claude"`, `"vibe"`), and
  the `feature_presets` docs now list the real field set. Nothing about AMF's
  behavior changes.
- **Review and Plan are no longer labeled "(experimental)".** Both features
  are stable and fully documented in the README, so the feature-wizard
  toggles, the batch-creation dialog, feature-list and view-mode badges, and
  the leader-key help all drop the caveat. Steering keeps its experimental
  label for now.
- **`amf --help` explains itself.** The top-level help text now describes
  what AMF is, and `-V, --version` has a one-line description instead of
  showing up blank.
- **AMF now respects `XDG_CONFIG_HOME`.** Config, plugins, notifications, and
  the project database resolve through `dirs::config_dir()` instead of a
  hardcoded `~/.config/amf`. Existing installs are unaffected: if the old
  `~/.config/amf` already has data and the new location doesn't, AMF keeps
  using the old path, so nothing moves out from under you on upgrade.
- **Stopped features restart with only one saved agent by default.** When you
  start a stopped feature that has several saved Claude, Codex, opencode, or Pi
  panes, AMF now auto-launches only the first agent harness and leaves the rest
  as tmux windows at the shell prompt. This avoids suddenly reviving a large
  batch of agent CLIs on memory-constrained environments such as WSL. Set
  `max_agent_autostart_sessions` to `0` to restore unlimited auto-start.

### Fixed

- **Posted AI-review findings stay actionable in PR Triage.** After posting an
  AI review with `W`, its findings now remain available for the normal
  mark-done, reply, not-needed, and thread-resolution workflow instead of
  remaining trapped as local drafts. Refreshing or reopening the PR preserves
  that state without duplicating the posted comments, and a temporary refresh
  failure cannot cause a second `W` to post the same review again. No migration
  is required.
- **Reusing a feature branch now opens its current PR.** When the same branch
  has an older closed or merged PR and a newer open one, PR Triage and the
  dashboard badge now follow the open PR instead of restoring the closed
  predecessor. Returning from a linked triage session also cannot reopen stale
  state from the earlier PR. Closed PRs remain available through explicit
  selection, and their saved triage history is preserved. No migration is
  required.
- **A failed `W` post no longer boots you out of the PR-review pane.**
  An AI finding whose reported line does not actually exist in the PR diff is
  now included in the review summary instead of being sent as an invalid
  inline comment that makes GitHub reject the entire review. If posting still
  fails — for example, because the PR changed after the review was generated —
  the post-confirm dialog stays open with the failure shown inline instead of
  silently kicking you back to the dashboard, and a rejected-review (422)
  failure gets an actionable message instead of GitHub's raw status line.
- **Headless Claude calls no longer fail on large prompts.** `ClaudeLauncher`
  passed the prompt as a `-p <prompt>` command-line argument; Linux caps a
  single argument at 128 KiB (`MAX_ARG_STRLEN`), well under what a real PR
  diff or file review routinely runs to, so any prompt past that failed the
  whole spawn with `E2BIG` ("claude headless command failed") before Claude
  ever saw the request. The prompt is now piped over stdin instead, which has
  no comparable ceiling. Affects every headless caller — the PR-review AI
  review (`A`), the review-memory lookback bootstrap, final review's diff
  walkthrough/co-review/changeset overview, and session summaries.
- **Sending pasted images from Windows/WSL now shows progress.** The composer
  stays visible with `[Pasting...]` while AMF moves an attached image through
  the Windows clipboard and waits for the agent harness to ingest it, so the
  app no longer appears frozen during the slower PowerShell handoff.
- **Claude sidebar TODOs stay tied to the selected session.** When another
  Claude session has a newer task list, AMF no longer shows that unrelated
  checklist in the current session's sidebar. The sidebar still uses the
  selected session's own task store or transcript, so TODO progress remains
  session-specific.
- **Agent sessions start reliably on macOS.** Claude, Codex, opencode, Pi,
  custom sessions, and related launch helpers no longer paste long environment
  setup commands directly into tmux panes, so macOS no longer cuts off the
  command before the agent starts.
- **macOS source builds work again.** AMF now uses the libc argument types
  expected by macOS for the tmux PTY setup, so `cargo build` no longer fails
  with `openpty` / `ioctl` type errors on that platform.
- **AMF-managed hooks now stay dormant outside AMF.** Running Claude, Codex, or
  Opencode directly from a worktree that AMF has prepared no longer triggers
  AMF's notification, thinking, sidebar, or diff-review hooks. This prevents
  standalone Claude runs from getting stuck waiting for an AMF diff-review
  prompt that is not open.
- **Claude hooks recover from stale temporary AMF config paths.** If an AMF
  verification run or unusual `HOME` / `XDG_CONFIG_HOME` setting left Claude
  hooks pointing at deleted `/tmp/claude-*` helper scripts, AMF now removes
  those stale entries and reinstalls the current local hooks on startup. This
  clears repeated `PostToolUse:Bash hook error` messages without requiring
  manual edits to `.claude/settings.local.json`.
- **Claude hook paths with spaces work on macOS.** AMF now quotes its Claude
  hook commands and cleans up older unquoted macOS hook entries, so paths under
  `~/Library/Application Support/amf` no longer fail with `/bin/sh` errors
  such as `Library/Application: no such file or directory`. Existing features
  with stale local Claude settings are repaired during AMF startup, even if the
  normal hook refresh already ran.
- **File-level PR review comments no longer inject the whole file as a diff
  hunk.** A comment left on the file as a whole (GitHub's `subject_type:
  "file"`) carries the entire file diff as its `diff_hunk` — fixing it used to
  paste that whole diff into the fix prompt. The fix prompt now references
  `File: path` instead and omits the hunk, with the same backstop for any
  line-anchored comment whose hunk is pathologically large (over 150 lines,
  well clear of the ~90-line hunks ordinary line comments run to).
- **Final-review line comments no longer disappear when the diff changes.**
  Refreshing the diff — or switching the base ref — used to drop your line
  comments out of the gutter, because each one was pinned to an exact line
  number. Comments now follow their code when it moves, including multi-line
  ones. If a comment's line is genuinely gone (the agent already fixed it, say),
  AMF tells you it was "possibly addressed" instead of silently discarding it,
  keeps it in the feedback file, and won't post it to a pull request where it
  would land on the wrong line.
- **Markdown tables show their header rows again.** The in-app Markdown viewer
  now renders table headers and the header/body divider, so plan and review
  docs with tables are readable instead of starting at the first body row.
- **Markdown table alignment is now guarded against regressions.** The in-app
  Markdown viewer keeps left, centered, and right-aligned columns stable,
  including when narrow panes force table cells to truncate.
- **Markdown tables keep uneven and empty cells in place.** Short rows are
  padded to the table's declared column count and empty cells still occupy
  visible grid space, so later cells no longer appear to shift left in the
  in-app Markdown viewer.
- **Markdown tables keep their quote and list indentation.** Tables rendered
  inside blockquotes or list items now preserve the surrounding prefix on every
  border and row, and narrow prefixed tables still clamp to the available
  viewer width.
- **Markdown tables keep cell styling and stay readable when cells are long.**
  Inline code, emphasis, strong text, strikethrough, and links now keep their
  styling inside table cells, and oversized cells truncate with an ellipsis
  without breaking the table borders.
- **Markdown footnotes are readable in the in-app viewer.** Footnote
  references now show as compact inline labels, and definitions render as
  labeled footnote blocks instead of exposing raw `[^label]:` syntax.
- **Markdown math is visible in the in-app viewer.** Inline and display
  formulas now render as styled `$...$` / `$$...$$` text, including inside
  table cells, so plans and notes with formulas no longer look like ordinary
  prose or lose math-specific context.
- **Agent harness launches hide their startup command echo.** When AMF starts a
  fresh Claude, Codex, opencode, or Pi session — including Pi opened from a
  feature row, and Claude/Codex/opencode sessions resumed with `S` — the
  embedded pane now shows a loading screen until the harness is ready instead
  of flashing the long tmux launch command and environment setup. Existing
  running sessions open normally.
- **Terminal sessions no longer add surprise blank lines on macOS.** AMF now
  uses the direct tmux input path on macOS, so typing in an embedded terminal
  inserts the intended characters instead of turning each keypress into a
  newline.
- **Dialog footers no longer promise the wrong key.** The first-run harness
  wizard said `c confirm  Esc confirm` — Esc actually confirmed, not
  canceled — so it now reads `c/Esc done`. The New Project dialog said
  `Enter confirm` on every field, but Enter only confirms after the last
  one; it now reads `Enter next` on the Name and Repo path fields and
  `Enter confirm` only once you're on the harness field.
- **Empty project list is no longer flush against the left border.** The
  "No projects yet." message now has the same left padding as every other
  row.
- **The existing-worktree picker in the new-feature wizard now scrolls.**
  When a project has more worktrees than fit in the dialog, navigating past
  the visible rows used to scroll the selection out of view with no way to
  see it again. The list now auto-scrolls to keep the selected worktree
  visible, like every other picker in AMF.
- **Inline PR comments now show the code they actually reference.** GitHub can
  attach an entire newly-added function to a comment on one line, making the
  relevant code hard to find and bloating fix prompts. AMF now centers the
  displayed and injected diff on the referenced line with three surrounding
  lines of context on each side, including for outdated and cached comments.

### Migration

- No migration is required for the PR Triage rename. Existing dedicated
  `PR Review` sessions continue to work and are reused automatically.
- No migration is required for focused PR-comment diff context; cached reviews
  adopt it automatically when displayed.
- No migration is required for the Windows/WSL image-paste progress indicator.
- No action required for comment re-anchoring. A review you paused before
  upgrading still resumes; its comments simply can't follow moved code until you
  re-add them, and any that no longer match are flagged rather than dropped.
- No action required for thread resolve/unresolve. Existing progress and
  snapshot files load with every comment treated as open (unresolved, not
  carried over), which is the same behavior they had before this release.
- No action required for the Markdown table alignment coverage.
- No action required for the Markdown table header fix.
- No action required for the Markdown table styling, table truncation, or
  footnote rendering fixes.
- No action required for the startup loading-screen fix.
- No action required for the macOS terminal input fix.
- No action required for the Review/Plan experimental-label graduation,
  the `--help` text, or the dialog footer and empty-state fixes.
- If you relied on `n` / `p` to move between features in a session, add
  `"next_feature"` / `"prev_feature"` keybindings to your config to restore
  them.
- If you want stopped features to resume every saved agent pane automatically,
  set `"max_agent_autostart_sessions": 0` in `~/.config/amf/config.json`.

## [v0.29.0] - 2026-07-01

### Added

- **AI co-reviewer first pass in a final review.** Press `A` to have Claude do
  a first pass over the file you're looking at and suggest line comments, so you
  start from a draft instead of a blank diff. Suggestions show up as *draft*
  comments — a hollow `○` in the gutter (vs the filled `●` of a comment you've
  kept) — and you adjudicate each one: with the line cursor active (`c`), `a`
  accepts the draft under the cursor, `d` dismisses it, `Tab` jumps to the next
  one, and `⏎` opens it to edit (editing also accepts). Drafts you don't accept
  are ignored — they never reach the feedback file or a posted PR review — but
  they do survive pausing and resuming a review. It runs only when you ask and
  only on the current file (with the diff capped), so it stays cheap on tokens.
- **Suggested changes in a final review.** With the line cursor active (`c`),
  press `S` to propose a concrete replacement for the line (or `v` range) you're
  on — the editor opens pre-filled with the current code so you tweak it rather
  than retype it. A suggestion can stand alone or ride along with a comment on
  the same line(s). It's written into the feedback file as a fenced
  ` ```suggestion ` block — a verbatim patch the agent can apply directly
  instead of interpreting prose — and, when you have PR posting on, it's
  appended to the inline PR comment as a GitHub suggestion so it's
  one-click-appliable on the pull request.
- **Jump between hunks while commenting in a final review.** With the line
  cursor active (`c`), press `]` / `[` to jump straight to the next / previous
  hunk instead of holding `j`/`k` through a long file — handy for skimming a big
  diff change-by-change. If the cursor is off, the first press turns it on (`]`
  on the first hunk, `[` on the last), and from the middle of a hunk `[` snaps to
  that hunk's top first. A range you're marking with `v` carries across the jump,
  so you can still select a span that spans hunks.
- **A line comment now flags its file as needing work.** Leaving a comment (or a
  suggested change) on a file in a final review is itself a signal the file
  isn't done, so AMF now marks that file "needs revision" for you — no separate
  reject step, and the comments themselves stand in as the reason. Your own call
  always wins: explicitly approving, skipping, or rejecting a file overrides the
  automatic mark and sticks, and clearing a file's last comment clears an
  automatic mark again. Accepting an AI draft comment counts the same as writing
  one; dismissing a draft doesn't. The distinction is remembered if you pause and
  resume a review.
- **Per-project TODO lists.** Add a `TODOs` session from the session picker
  (`s`) to keep a running to-do list for a project — somewhere to jot what's
  left and where you left off instead of holding it in your head. It opens a
  native full-screen list (no tmux pane) with a free-form **scratchpad** note
  at the top (for context, links, or where you left off), and each project gets
  at most one list, shared across all its features. Inside the list: `a` add,
  `e` edit a title, `o` edit longer notes on an item, `space` toggle done
  (completed items get a strikethrough and sink to the bottom, never
  auto-cleared), `p` cycle priority, `J`/`K` reorder, `d` delete (with confirm),
  `b` edit the scratchpad note, and `j`/`k` to move. Press `g` (or `Enter`) on
  an item to **launch an agent for it**: AMF opens a fresh agent session in the
  list's feature — using that feature's harness and mode — and pre-fills the
  composer with a prompt built from the item's title and notes, left editable so
  you review it before sending. Launched items show a marker, and pressing `g`
  again jumps back to the same session (adding onto it rather than spawning a
  second). You can also **quick-capture** a TODO without leaving whatever session
  you're in: from a session view, leader → `N` opens a one-line input that
  appends to the project's list (auto-creating the list if there isn't one yet).
  The list is saved per checkout, so it survives restarts.
### Fixed

- **Notification hook scripts are written atomically.** AMF rewrites its
  helper hook scripts (`tool-start.sh`, `notify.sh`, and friends) under
  `~/.config/amf` on startup. Those writes happened in place, so if a script
  changed (e.g. after an upgrade) at the moment the agent harness was executing
  it, the shell could read a half-rewritten file and fail with a spurious
  syntax error — which surfaced as a hook error blocking a tool call. AMF now
  stages each script in a temp file and renames it into place, so a running
  hook always sees a complete file (old or new, never a mix).

- **PR-comment triage marks no longer vanish after the agent pushes a fix.**
  When reviewing PR comments, marking a comment done/skipped, injecting a fix,
  or posting a reply records local triage state — but that state was keyed by
  the PR's head commit, so as soon as a fix was pushed and you reopened the
  review (`G`), the new head SHA meant none of your marks showed up. Triage is
  now keyed by the comment itself and survives a push, so done/skipped/fixing
  marks and skip notes persist across commits and re-opens. Existing triage is
  migrated automatically (any per-commit duplicates collapse to your latest
  mark).

- **Agent usage now binds to the right session more reliably.** New Claude,
  Codex, and opencode panes now pass enough session identity through their
  local hooks/plugins for AMF to attach token usage to the matching dashboard
  session instead of guessing from the newest transcript in the worktree.
- **Inferred usage is safer for older sessions.** When AMF has to fall back to
  workdir/timestamp matching, it no longer assigns the same inferred provider
  session to multiple same-harness panes in one feature; unmatched sessions stay
  unbound until a better match or exact provider event appears.
- **New Codex panes no longer inherit old usage.** A newly created Codex pane
  could briefly show a large cost from an older Codex thread in the same
  worktree before Codex emitted its exact session identity. AMF now ignores
  stale inferred Codex sources and clears any older bad inferred binding on the
  next refresh, so new panes stay blank until their own usage is known.

### Changed

- **Readable comment rows in the interactive PR review.** Each row now leads
  with the **reviewer's name** — bold and in the accent color — so you can scan
  who left each comment at a glance. Long file paths no longer hide the part
  that matters: the location is truncated from the *left* with a leading `…`, so
  the filename and line number stay visible even when the path is too long for
  the pane (e.g. `@reviewer  …/dialogs/pr_review.rs:123`).
- **Feature rows now show total agent usage.** The dashboard feature row shows a
  compact feature-level usage total, while each agent session row and sidebar
  keeps showing only that specific pane's usage. Terminal, editor, custom, and
  unsupported Pi sessions are excluded from the feature total.
- **A real editor for the PR-comment fix prompt.** The confirm dialog that
  shows the scoped fix before it's injected (press `f` while reviewing PR
  comments, then `e` to edit) is now a full editor instead of a plain text
  box. Toggle **vim keys** with `Ctrl+T` — the choice sticks for the rest of
  the PR, so reopening the dialog for the next comment keeps your keymap, and
  the title shows `· vim insert` / `· vim normal` so you know whether `Esc`
  leaves the dialog or just drops to normal mode. Long prompts now **scroll**
  (`Ctrl+J`/`Ctrl+K`, `PgUp`/`PgDn`, with a scrollbar) and follow the cursor as
  you type, and you get undo/redo and word motions for free. **`Tab` injects**
  the prompt straight from edit mode (where `Enter` makes a newline); `Enter`
  still injects from the confirm view.
- **Consistent vim-toggle key across every editor.** All the multi-line editor
  dialogs now toggle vim mode with the same key, **`Ctrl+T`**. Previously the
  steering prompt, the diff-review feedback editor, and the PR-comment fix
  prompt used `Ctrl+V` while the compose box, prompt library, and fill-in
  fields used `Ctrl+T`. They're unified on `Ctrl+T` — which also frees `Ctrl+V`
  from clashing with paste. Footers, the status bar, and the help screen all
  reflect the new key.

### Migration

- No migration is required. Existing inferred usage sources can be replaced
  automatically when the next exact provider event arrives, and stale inferred
  Codex usage is cleared on refresh.

## [v0.28.0] - 2026-06-29

### Added

- **Run final-review fixes in a fresh session.** Press `t` in a final review to
  choose where the "address this feedback" prompt goes when you finish: the
  feature's existing agent pane (the default, as before) or a brand-new
  dedicated review session — the footer shows `t target: live` / `dedicated`.
  Pick the dedicated target and, when finishing needs to spin up that session,
  AMF asks which harness should run it (Claude / Codex / opencode / …) so the
  fixes run in a clean context instead of the long-running review conversation.
  A dedicated "Final Review" session is reused on later rounds without asking
  again. The feedback file is always written first, so skipping the harness
  picker just leaves it for later.
- **Multi-line comments in a final review.** A line comment can now cover a range
  of lines, not just one. With the line cursor active (`c`), press `v` to start a
  selection, extend it with `j`/`k`, then `⏎` to attach a single comment to the
  whole span. The selection is highlighted as you mark it, every line of a saved
  range carries the `●` gutter marker, and the anchor is recorded as
  `src/foo.rs:42-48` in `.claude/final-review-feedback.md`. When posting to a
  GitHub PR, a ranged comment becomes a proper multi-line review comment. Parking
  the cursor on any line of a range peeks the comment and re-opening it keeps the
  range so you can just edit the text.
- **Peek a line comment while browsing the diff.** In a final review, lines you've
  commented on already show a `●` marker in the gutter — now parking the line
  cursor on one also pops the comment's text into a "comment on this line" box
  above the footer hints, so you can re-read what you wrote without reopening the
  editor. Press `⏎` to edit it as before; long comments are previewed up to a few
  lines.
- **Final review can post to your GitHub PR.** Turn on
  `final_review_post_to_pr` and finishing a final review also posts the
  feedback to the branch's PR as a single GitHub review: your line comments
  land as inline comments on the diff, and whole-file rejections plus any
  general feedback go in the review summary. It posts as a plain `COMMENT`
  review (so it works on your own PR) and is best-effort — if there's no PR for
  the branch or `gh` isn't set up, AMF just says so and carries on. The local
  `.claude/final-review-feedback.md` is still written every time, so nothing is
  lost whether or not the PR post succeeds. Off by default; needs an
  authenticated `gh` CLI.
- **Final review flags what changed since last time.** Finishing a final review
  now records a fingerprint of the diff, so the next time you review the same
  feature AMF marks the files that changed since your last pass with a `Δ` in
  the file list (and a `Δ N changed` count in the header). On a fresh re-review
  where only some files changed, the file list automatically narrows to just
  those files and lands on the first one — press `F` to cycle the filter
  (all → undecided → rejected → changed) and see everything again. This closes
  the loop after an agent addresses feedback: you only re-check what it touched.
- **Reply to PR comments from inside AMF.** In the PR comment-review pane you
  can now answer a comment without leaving the tool, with two purpose-built
  replies: press `R` to post a "Done in `<sha>`." reply auto-filled from your
  latest commit (and mark the comment done), or `n` to write why a fix isn't
  needed (and mark it skipped, keeping your note). Both open an editable draft
  and only post when you press `⏎` — replies to inline review comments land in
  the right thread, while conversation comments and review summaries post to the
  PR conversation. The first time you post, if your `gh` login lacks write
  access AMF tells you to run `gh auth refresh -s repo` rather than failing
  cryptically.
- **Resolve PR review threads from the pane.** Press `x` on an inline review
  comment to resolve its thread on GitHub (or reopen one that's already
  resolved) without leaving AMF — the `✓` marker updates immediately. Resolving
  is independent of replying, so you can close out a thread with or without a
  comment. After you post a reply AMF also re-checks thread resolution, so the
  list stays in sync if a comment got resolved meanwhile. Conversation comments
  and review summaries have no thread to resolve and say so.
- **PR review triage state sticks.** In the PR comment-review pane you can now
  mark a comment done (`m`) or skip it (`s`), and injecting a fix (`f`)
  automatically flags the comment as "fixing". These states persist across
  re-opening the PR and restarting AMF, so a long review picks up where you left
  off. The comment list shows a per-comment checkbox (`[ ]`/`[~]`/`[x]`/`[-]`)
  and the detail header a `[fixing]`/`[done]`/`[skipped]` chip — distinct from
  GitHub's own `✓` thread-resolution marker. Mark-done and skip are manual with
  no auto-advance: the selection stays put so you can watch the agent work.
- **The PR review pane is much easier to read.** The comment detail is no longer
  a flat wall of text: it's split into clear sections (header, diff hunk, body,
  and any local note) separated by subtle dividers, with author/role/kind/
  resolution/triage shown as compact chips and a marker legend in the footer.
  Comment bodies render as Markdown — headings, lists, code blocks, and inline
  code instead of raw text — and the diff hunk is colored like a diff (added
  green, removed red) **and syntax-highlighted** for the comment's language
  using the same tree-sitter highlighter as the diff viewer. When a language's
  parser isn't installed it falls back gracefully to plain coloring.
- **Install syntax highlighting without leaving the PR review pane.** When a
  comment's diff hunk falls back to plain coloring because its language parser
  isn't installed, the pane now shows an inline `<Lang> highlighting not
  installed — press i` hint. Press `i` to open the syntax-language picker for the
  selected comment's file — install the parser and you're dropped right back into
  the same comment, now highlighted. This matches the `i` shortcut already in the
  diff viewer. The pane footer and the keybindings help (which gained a dedicated
  "While reviewing PR comments" section) list the new key.
- **Pick a PR to review from a list.** When you press `G` on a feature whose
  branch has no detectable PR — or press `g` inside the review pane to switch PRs
  — AMF now shows a scrollable list of the repo's pull requests (number · title ·
  author · branch, newest first) so you can open one without knowing its number.
  Press `a` to include closed/merged PRs, `#` to type a number instead (the old
  prompt is still one keypress away), and `⏎` to open the highlighted PR. The list
  is fetched in Rust via `gh pr list`, so it spends zero agent tokens.
- **Choose which harness runs your PR-review fixes.** The first time you press
  `f` to fix a comment in a PR, AMF now asks which agent harness the dedicated
  review session should run — so you can triage on a cheaper or faster harness
  than the one your feature is being built with. It highlights the project's
  preferred agent by default; pick one with `j/k` and `⏎` (or `esc` to back
  out). Your choice is remembered for the rest of that PR, so every later fix
  reuses the same warm session without asking again. Only the dedicated review
  session prompts — fixing in your existing live session keeps using whatever
  harness it's already running.
- Added the full Gruvbox Material UI theme family to the theme picker and
  config: dark/light, hard/medium/soft contrast, and material/mix/original
  foreground palettes. Use names like `gruvbox-material-dark-medium` or
  `gruvbox-material-original-light-soft` in `~/.config/amf/config.json`.

### Changed

- Embedded-view auto refresh is now configurable with `view_auto_refresh` and
  defaults to off. Manual refresh and event-driven pane updates still work; turn
  the setting on only if you still need periodic Claude/tmux visual repairs.

### Fixed

- AMF now cleans up the bundled `amf-gruvbox` Opencode theme along with the
  other AMF-managed Opencode themes when removing local Opencode integration
  files.
- Claude panes now render immediately behind the composer when an agent session
  opens with composer input enabled. Previously the composer could appear over a
  blank pane until you closed it or sent input.
- **Composer pastes now show progress.** Pressing `Ctrl+V` in the composer
  immediately shows `[Pasting...]` while AMF reads the clipboard, so slow text
  or image pastes no longer look like the app ignored the keypress.

### Migration

- No action required. AMF adds a `pr_comment_triage` table to its SQLite store
  on first launch; triage state is keyed by PR number, comment id, and head
  commit, and stale rows are pruned automatically after a week. The new
  Gruvbox Material themes are available immediately after upgrade.

## [v0.27.0] - 2026-06-25

### Added

- **Final review reads like a real code review.** Developer notes beside each
  diff now render as Markdown, so headings, lists, and code blocks in your
  `.claude/review-notes.md` show up formatted instead of as raw text.
- **Multi-paragraph review feedback.** The rejection and general-feedback
  editors in the final review are now full multi-line editors: press `Enter`
  for a new line and write proper paragraphs or lists, then `Tab` to submit.
  `Ctrl+V` toggles vim keybindings and `Esc` (or `Ctrl+Q`) cancels.
- **Paste-vs-submit toggle for the final-review prompt.** The new
  `final_review_submit_prompt` config (default `true`) controls whether
  finishing a review with feedback auto-submits the "address the feedback"
  prompt to the feature's agent. Set it to `false` to paste the prompt without
  sending Enter, so you can eyeball or edit it before submitting.
- **Line-level comments in the final review.** Press `c` in the patch to turn
  on a per-line cursor, move it with `j`/`k` (`g`/`G` jump to the first/last
  changed line), then `Enter` to attach a comment to that exact line — the
  cursored row and any commented lines are marked in the gutter (`▶`/`●`).
  Comments are written into `.claude/final-review-feedback.md` under a
  `## Line Comments` section anchored as `### <file>:<line>`, so the agent gets
  precise, location-specific feedback that whole-file rejection can't express.
  `Esc` exits the cursor; `q` still finishes the review. (Inline markers are
  shown in the unified diff layout.)
- **On-demand AI walkthrough for noteless files.** When a file in the final
  review has no developer note in `.claude/review-notes.md`, press `w` to
  generate a concise walkthrough of that file's diff with a headless Claude
  run. The Developer Notes panel shows "Generating walkthrough…" while it works
  and then renders the result as markdown (titled "AI Walkthrough"), cached per
  file so navigating away and back doesn't re-run it — so the notes panel is
  never empty even when the developer didn't leave a note.
- **Finish gating + jump-to-next-undecided in the final review.** Finishing a
  review (`q`/`Esc`) while one or more files still have no verdict now asks for
  confirmation instead of ending silently — press `q`/`y` to finish anyway,
  `Esc` to keep reviewing, or `u` to jump straight to the next file with no
  verdict (wrapping). The footer shows how many files are still undecided, so a
  half-done review isn't finished by accident.
- **Pause and resume a final review.** Your verdicts, line comments, and
  general feedback are now saved to `.claude/final-review-progress.json` as you
  go, so a long review survives quitting AMF (or a crash). Reopening the final
  review for the same feature restores everything where you left off; finishing
  the review clears the saved progress so the next one starts fresh.
- **Choose the diff base ref.** Press `b` in the branch diff viewer or final
  review to compare against any branch, tag, or commit instead of the
  auto-resolved base (`origin/HEAD` → `main` → `master`). The header shows a
  `(manual)` marker while an override is active; submitting a blank entry
  reverts to automatic resolution.
- **Final review keeps a history.** `.claude/final-review-feedback.md` is no
  longer overwritten each round — every review is now appended as a dated
  `## Review — <timestamp>` section, with the newest round first under a single
  title, so you keep a trail across rounds. The agent is prompted to address
  only the most recent round, so accumulated history doesn't make it re-do
  already-fixed items.
- **Filter the final-review file list.** Press `F` in the final review to cycle
  the file list through all → undecided → rejected → all, so a large changeset
  can be narrowed to just the files still needing attention. The Files panel
  title shows the active filter and visible/total counts, navigation
  (`n`/`p`, `j`/`k`, `g`/`G`) skips hidden files, and approving/rejecting a file
  advances to the next visible one.

- **Insert an agent skill into a prompt.** While editing a prompt body (or
  filling a text placeholder), press `Ctrl+K` to open a search-as-you-type
  picker of the agent skills available in your workspace — the same
  `~/.claude/skills` and project `.claude/skills` the compose `/command` popup
  draws from. Selecting one inserts its `/skill-name` invocation at the cursor,
  so the agent expands the skill when the prompt is delivered.

- **Vim editing in the prompt library.** The New/Edit Prompt body editor now
  supports a vim keymap you can toggle with `Ctrl+T` (just like the compose
  box), and the box title shows `[Vim Insert]` / `[Vim Normal]` so you always
  know which mode you're in. Multi-line placeholder fill fields gain the same
  `Ctrl+T` toggle, and the choice carries across slots while you fill them in.

- **Prompt library shows where each template lives.** The picker preview pane
  now displays the resolved source file for the selected entry — the local
  SQLite store for your own (`User`) templates, or the `.amf/config.json` path
  for `Project` / `Worktree` / `Global` templates. The export prompt (`x`) now
  names each target's exact path before you choose — e.g.
  `(g) ~/.config/amf/config.json   (p) <repo>/.amf/config.json` — so you can
  confirm which repo a template lands in. The New/Edit dialog adds a hint
  showing where a save will be written, calling out that `User` templates live
  in the local store and aren't version-controlled.

- **`/amf:add-prompt` skill.** A new skill (parallel to `/amf:add-preset`) that
  adds a declarative prompt template to your workspace without hand-writing
  JSON. It writes a `prompt_templates` entry to `.amf/config.json` (project) or
  the global `~/.config/amf/config.json`, and documents the template schema —
  including tags, inline `{{slot}}` / `{{name|opt1|opt2}}` syntax, and explicit
  text / multi-line / select placeholders. The template then shows up in the
  prompt library picker as a read-only `Project`/`Global` entry.

- **Tags for prompt templates.** The New/Edit Prompt dialog now has a `Tags`
  field (Tab cycles Name → Tags → Body); enter a comma- or space-separated
  list and AMF stores it with the template. Tags show as `#chips` in the
  picker's preview pane. In the picker's search (`/`), the query now also
  matches tags, and a `#`-prefixed query filters by tag only — `#frontend`
  narrows to templates tagged `frontend`, and a bare `#` lists every tagged
  template.

- The vim-mode editor now accepts numeric count prefixes, so you can repeat
  motions and edits the way you would in vim: `3w`, `5j`, `2dd`, `d3w`, and
  combined counts like `2d3w`. A leading `0` still jumps to the start of the
  line unless you are already typing a count (`10w`).

- **Review a PR's comments inside AMF.** Press `G` on a feature in the
  dashboard to open a full-screen pane listing the open pull request's
  comments for that branch — inline review comments, review summaries, and
  conversation comments together, each showing its file/line, author, a
  one-line preview, and whether the thread is resolved. Navigate with
  `j`/`k` to read the full comment and its diff context, scroll long comments
  with `Ctrl+D`/`Ctrl+U` (or `PgDn`/`PgUp`), and press `h` to hide comments
  already resolved on GitHub so you can focus on what's still open (a counter
  shows how many are hidden); `esc`/`q` closes. If the branch has no open PR,
  AMF prompts for a PR number instead of giving up, and you can press `g` from
  inside the pane any time to pull up a different PR's comments by number (a bad
  number reports the error inline so you can correct it). Everything is fetched
  with the GitHub CLI and stays read-only for now — acting on comments (sending
  a fix to the agent, replying) is coming in a later release. Requires the `gh`
  CLI installed and authenticated. Re-opening a PR whose latest commit hasn't
  changed is now instant and makes no network calls — comments are cached
  locally per PR and head commit — and pressing `r` in the pane refreshes from
  GitHub (picking up new comments and any commits you've pushed) on demand.

- **Send a review comment to an agent as a fix.** In the PR-review pane, press
  `f` on a comment to turn it into a tightly-scoped fix prompt for an agent
  session. `f` now opens a confirm dialog first that shows the exact text that
  will be sent — the comment, its `file:line`, and GitHub's diff hunk, with no
  file contents — alongside a `~N tokens` estimate and the target session, so
  nothing reaches the agent until you approve it. Press `e` to edit the prompt
  in place before sending (`Esc` back to the confirmation), `Enter` to inject,
  or `Esc` to cancel. On inject AMF drops you into the session to watch it
  work; the prompt is pasted without auto-sending so you can still eyeball it
  there. By default the fix goes to a dedicated "PR Review" session that AMF
  spins up once and reuses for every fix on that PR (so the per-session
  overhead is paid once); press `t` to switch the target to the feature's
  existing live session for warm in-progress context. The footer shows the
  current target (`f fix→dedicated`).

### Changed

- **Clearer feature setup options.** The new-feature mode step now shows a
  distinct details callout when you hover or focus optional settings like
  Review, Plan, Chrome, Remote Control, and Steering, making the tradeoffs
  easier to understand without crowding the mode picker.

- **New/Edit Prompt dialog redesign.** Each field (Name, Tags, Prompt body)
  now sits in its own labelled box with clear spacing instead of stacking
  flush against each other. The focused field's border highlights, the
  placeholder-syntax legend moved onto the body box's bottom border, the key
  hints moved onto the dialog's bottom border, and the "where this saves"
  hint sits at the bottom — so the form is much easier to scan.

- The AMF prompt composer now works in Codex, OpenCode, and Pi sessions with
  the same workflow and keybindings as Claude Code. Start typing to compose,
  use `Ctrl+E` or `leader+e` to switch between compose and direct input, and
  use drafts, prompt templates, multiline input, clipboard paste, and image
  attachments without changing workflows between agent harnesses. Terminal,
  editor, and custom sessions continue to use direct input.
- Opening an agent session now shows the composer immediately when composer
  input is enabled, so you can start drafting without typing a throwaway first
  character. Sessions switched to direct input still open directly into the
  pane.

### Fixed

- Claude sidebars now find todo lists that Claude stores under a separate
  task-store ID instead of the visible session ID, so the sidebar's `Todos`
  panel shows the current checklist again.
- Pasting now works in the prompt library editor. Bracketed-paste events
  were dropped in the New/Edit Prompt dialog and the placeholder fill flow,
  so clipboard text never landed in the name, tags, body, or fill fields.
  Pastes now route to the focused single-line field (newlines collapsed) or
  the multi-line body/fill editor.

- Dialogs opened from a feature now keep a compact project/feature/session
  context bar visible, so you can still tell which workspace you are acting on
  while reviewing diffs, editing prompts, or using other overlays.

- Pasting images into the prompt composer now works on Windows/WSL. The
  composer reads screenshots and copied images from the Windows clipboard,
  and when you send a prompt the image now reliably arrives with your text
  instead of being left behind in the agent's input box.
- Claude panes now keep their periodic automatic repaint when direct tmux
  transport is active, preventing the embedded UI from becoming mangled on
  Linux even though control mode is disabled by default.

- Adding a session to a stopped feature now starts the feature automatically,
  so the new session opens immediately and the dashboard returns to a green
  status. If AMF cannot start the feature or create the session, it now shows
  an error toast instead of appearing to do nothing.

- Scrolling to the bottom of a developer note in the final review now reaches
  the true visual bottom. Long notes that soft-wrap or expand under Markdown
  rendering were clamped to the raw line count, so the last lines stayed out
  of view; the scroll limit now tracks the rendered (wrapped) height.

### Migration

- No migration is required. The composer is enabled by default for all
  built-in agent harness sessions; use `leader+e` per session to opt into
  direct input.

## [v0.26.0] - 2026-06-22

### Added

- **Fill-in placeholders for prompt templates.** Injecting a template that
  contains `{{slot}}` markers now walks you through one field at a time,
  collecting a value for each slot before the prompt is delivered. A `2/3`
  counter shows your progress; `Tab`/`Enter` move to the next field,
  `Shift+Tab` goes back, and `Ctrl+S` injects at any point. Slots are filled in
  the order they appear in the prompt, pre-seeded with any configured defaults,
  and required slots block injection until filled. Templates without slots
  inject immediately as before.

- **Menu (pick-from-a-list) placeholders.** A slot can offer a fixed set of
  choices instead of free text: when you reach it in the fill-in flow, you pick
  from a list with `↑`/`↓` (or `j`/`k`) and `Tab` to confirm. Define one right
  in the prompt body by listing the choices with `|` — `{{a|b|c}}` makes every
  segment a selectable option, for example `Deploy to {{dev|staging|prod}}`. Add
  an optional heading with a leading `label:` — `{{env: dev|staging|prod}}` shows
  "env" above the same three choices. Plain `{{name}}` stays a free-text slot.
  The New/Edit Prompt editor now shows a short, always-visible help line
  explaining all three forms (it no longer disappears once you start typing).

- **Save a past prompt to the library.** In the recent-prompts menu
  (`leader+L`), press `s` to save the highlighted prompt as a new reusable
  template — it opens the editor pre-filled so you can name and tweak it.

- **Worktree export target for prompt templates.** Pressing `x` → `w` in the
  prompt library exports a template to the active feature's worktree
  `.amf/config.json` instead of the main project config. Worktree templates
  appear with a distinct yellow `[Worktree]` badge so you can tell them apart
  from `[Project]` templates. This lets you commit a prompt on a feature branch
  and promote it to the main repo via git when it's ready.

- **Edit prompts from any source.** Project, Global, and Worktree templates can
  now be opened in the prompt editor with `e`. Saving writes back to the config
  file they came from — SQLite for `[User]` templates, the appropriate
  `.amf/config.json` for config-file sources. Renaming a config-source template
  removes the old entry so stale names don't accumulate.

- A running bug backlog in `docs/backlog/bug-backlog-plan.md`, viewable
  in AMF's in-app Markdown viewer alongside the existing plans, so known
  issues are tracked in one place. First entry: pressing `A` on the
  dashboard does not open harness setup.

### Changed

- All four template sources (User, Worktree, Project, Global) are now shown
  independently in the prompt library with no cross-source deduplication.
  Previously, a Worktree or Project template with the same name would silently
  hide the Global copy, making freshly-exported templates appear to vanish.
  Each scope's copy is now always listed so you can see exactly where a prompt
  lives and manage them individually.

- The library list updates immediately after every mutation — export, create,
  edit, or duplicate — and the cursor jumps straight to the affected entry.
  Previously the list reset to position 0 and newly-exported entries could be
  invisible until you closed and reopened the picker.

- Global templates are re-read from disk each time the library opens, so
  changes made while AMF is running (including exports from the same session)
  take effect without restarting the app.

- Export failures now surface as a toast instead of doing nothing silently.
  A corrupt or unwriteable config file shows an explicit error message.

- Only `[User]` templates show the `d` delete hint. Config-source templates
  (`Project`, `Global`, `Worktree`) show `e edit` but not `d del`, since
  deleting those requires editing the config file directly. Pressing `d` on a
  config template now immediately explains this instead of arming a confirm
  dialog that then refuses to act.

- **Direct tmux transport is now the default.** AMF no longer attaches a
  persistent control-mode (`-CC`) client to drive agent panes; it talks to
  tmux directly, the same way it always has on Windows. This is what fixes the
  long-standing garbled-pane corruption (see Fixed). Control mode remains
  available as an opt-in via `tmux_control_mode: true` in `config.json` (or the
  `AMF_TMUX_INPUT_TRANSPORT` / `AMF_EXPERIMENTAL_PERSISTENT_TMUX_INPUT` env
  vars). The periodic automatic pane refresh and the SIGWINCH re-anchor bounce
  now run only in control mode, where they're needed — direct mode re-captures
  the full pane each frame and self-heals, so it no longer flickers from those.

- The diff-review popup now holds for **1.5s** by default instead of 3s.

  **Migration:** on first launch after upgrading, AMF rewrites `config.json`
  once to apply the new defaults — a persisted `tmux_control_mode: true` is set
  to `false` and a `diff_review_popup_hold_secs` of `3.0` becomes `1.5`. Values
  you previously customized away from those old defaults are left untouched, and
  the file is stamped with a `config_version` so the migration never runs twice
  (re-enable control mode afterward and it stays enabled).

- **Retired the legacy vimdiff diff-review viewer.** The native in-app AMF diff
  viewer is now the only reviewer; the neovim/vimdiff popup path and its bundled
  `plugins/diff-review` scripts have been removed. Existing configs that still
  set `diff_review_viewer = "nvim"` (or the old `"legacy"` alias) keep working —
  the value now deserializes to the AMF viewer instead of erroring.

### Maintenance

- Repo cleanup pass: removed dead code across the codebase (unreferenced tmux
  helpers, orphaned methods/constants, and a never-run test) and dropped the
  crate-wide `#![allow(dead_code)]` so new dead code is caught going forward.
  Items intentionally retained (write-only fields, serde/API types, and
  test-only helpers) now carry targeted `#[allow(dead_code)]` annotations. No
  behavior change; all tests pass.

### Fixed

- **Garbled / mangled agent panes on Linux and macOS.** The persistent
  control-mode tmux clients AMF attached to each session could desync Claude
  Code's incremental renderer, leaving stale cells in the pane grid (the input
  box bleeding into the divider above it). Windows was never affected because
  that transport is Unix-only. Defaulting to the direct transport removes the
  extra clients and the corruption with them.

- Diff/review mode no longer shows AMF-generated runtime files as if they
  were branch changes. The change-tracker log (`.amf/change-history.json`)
  and the opencode TUI state (`tui.json`) are now gitignored and untracked,
  so the review surface reflects only the real changes on your branch.

- Pressing `A` on the dashboard now opens the agent-harness setup picker as
  expected. The key was only wired into the `Ctrl+Space` leader chord, so a
  bare `A` did nothing. It is now a top-level dashboard action (and listed in
  the `?` help overlay) alongside the other capital-letter shortcuts.

## [v0.25.0] - 2026-06-19

### Added

- Prompt library (phase 1): save reusable prompts and inject them into a
  session on demand. Open the picker with `leader+P` while viewing a
  session, or `L` from the dashboard. `Enter` injects the selected
  prompt — into the compose box when compose is on (so you can review
  and edit before sending), or pasted straight into the agent window
  without sending when compose is off. Manage entries from the picker
  (`n` new, `e` edit, `d` delete, `y` duplicate, `/` search) or export
  one to declarative config with `x` → `g` (global
  `~/.config/amf/config.json`) or `p` (project `{repo}/.amf/config.json`)
  so it can be version-controlled and shared. From the compose box,
  `Ctrl+P` opens the library to inject a saved prompt (mirroring
  `leader+P`) and `Ctrl+S` saves the current buffer as a new template.
  Templates are
  stored in the AMF SQLite database. Fill-in `{{placeholder}}` slots and
  declarative/team templates are planned for later phases.
- Vim normal mode now supports the change operator: `c` with a motion
  (`cw`, `cb`, `c$`, …) deletes and drops into insert mode, `cc`/`S`
  changes a whole line (leaving it empty rather than removing it), and
  `C` changes to the end of the line. The delete and the text typed
  afterwards undo together as one step.

### Documentation

- Optimized the README for LLM/AI-assistant retrieval (c7score):
  merged install and upgrade steps into copy-paste-ready blocks,
  removed screenshot placeholders, and trimmed the duplicated `src/`
  file tree in favor of a pointer to `CLAUDE.md`.
- Fixed README issues found during the cleanup: a Catppuccin theme
  list and a `cargo build` snippet that had drifted into the wrong
  sections, and a dead link to a non-existent
  `docs/docker-screenshots.md`.
- Removed the `zai` (Z.AI usage limits) configuration section from the
  README. The feature is not reliable enough to document yet; the
  config key still exists for future use.
- Added a top-level `llms.txt` navigation file so AI tools can quickly
  locate the README, architecture docs, and automation guides.

### Changed

- Final review (press `f` in a feature view) now runs entirely inside
  AMF instead of opening tmux/vim popups. You step through every changed
  file with syntax-highlighted diffs, read the agent's own per-file
  reasoning beside each change (captured by review mode in
  `.claude/review-notes.md`), and approve, reject with feedback, or skip
  each file — plus leave general feedback that isn't tied to a file. On
  finish, AMF writes `.claude/final-review-feedback.md` and prompts the
  feature's agent to read it and address every item, so a review round
  flows straight back to the agent without copy-pasting.
- The per-file change review now finds a developer's note even when the
  heading includes a title (e.g. `## path — summary`), not only a bare
  path heading, so your review-mode notes show up more reliably.
- Feature view now highlights the active worktree name with a stronger
  accent color, making labels like `/visual-updates` easier to spot in
  the header.
- Renamed session bookmark internals and UI copy to remove legacy
  external-editor terminology.

### Fixed

- Final review no longer crashes with a tmux `open terminal failed: not
  a terminal` error. The previous version tried to attach a separate
  terminal session for its vim/popup walkthrough, which doesn't work
  with AMF's embedded tmux; the review now stays inside AMF.
- The Ctrl+Space leader menu now lists the bookmark shortcuts for
  opening bookmarks, adding or removing the current session, and jumping
  to saved bookmark slots, so the on-screen help matches the available
  commands.

### Migration

- The AMF database gains a `prompt_templates` table (schema migration
  `007`), applied automatically on first launch. No user action is
  required.

## [v0.24.0] - 2026-06-16

### Added

- Vim mode in the compose and steering-prompt inputs now supports undo
  and redo: press `u` in normal mode to undo and `Ctrl+R` to redo.
  Everything typed during a single insert session is undone in one step.
- Vim normal mode now supports operator + motion editing: `d` combined
  with a motion (`dw`, `db`, `de`, `d$`, `d0`, `dh`, `dl`), line
  deletes (`dd`, and `dj`/`dk` for multiple lines), `D` to delete to
  end of line, and the `e` motion to jump to the end of a word.
- Vim normal mode now supports yank and paste: `y` with a motion
  (`yw`, `y$`, …), `yy`/`Y` to yank a line, and `p`/`P` to paste after
  or before the cursor (charwise or linewise). Deletes and `x` feed the
  same register, so `ddp` and `xp` work as in vim.
- Remote Control: drive a feature's local Claude session from
  claude.ai/code or the Claude mobile app while the agent keeps running
  on your machine in tmux. Turn it on with the new "Remote Control"
  toggle when creating a Claude feature, or set it as the default for
  all new Claude features in config. The toggle disables itself with a
  reason when it can't be used — on z.ai or other third-party providers,
  or with a Claude Code older than v2.1.51 — so you never start a session
  that can't connect.
- A `[remote ●]` badge appears in the session view when Remote Control
  is connected, and RC-enabled features are marked `[remote]` in the
  project list. From a Claude view, the Ctrl+Space menu adds: `c` to
  copy the session URL, `O` to open it in your browser, and `C` to
  toggle Remote Control on or off at runtime. When the link only lives
  in Claude Code's footer (and can't be read back), AMF tells you to use
  that footer link rather than failing silently.

### Changed

- Review mode, final review, PR comment review, plan mode, and the steering
  coach now show an experimental label in the UI, so users can tell these
  workflows are still being refined before they opt into them.

### Fixed

- Opening the branch diff viewer and Markdown viewer now shows a
  loading indicator while AMF gathers files or reads content, so slower
  startups and large worktrees no longer look like the app ignored the
  command.

### Migration

- No migration is required.

## [v0.23.0] - 2026-06-15

### Added

- New compose input for Claude Code sessions that sidesteps Claude
  Code's input-box rendering glitches. Start typing in a Claude view
  and an AMF-drawn input opens over Claude Code's own input box; press
  Enter to send the finished text in one shot (Alt+Enter inserts a
  newline). Claude Code's output stays visible and live above the box
  while you type.
- Typing `/` in the compose input opens a slash-command menu listing
  Claude Code built-ins, your global and project custom commands, and
  skills, with descriptions. Arrows or Ctrl+P/N select, Tab completes,
  Enter runs. Commands that open Claude Code's own dialogs (such as
  `/model` or `/config`) automatically hand control back so you can
  drive them directly.
- Images can be pasted into the compose input with Ctrl+V. They show
  as `[Image 1]` placeholders and are delivered to Claude Code as real
  image attachments on send, so the agent can see them. Text on the
  clipboard pastes as usual.
- A direct-input escape hatch for when you want keys to go straight to
  Claude Code again: press Ctrl+E in the composer (or `leader+e` in
  the view) to disable the compose input per session, shown with a
  `[direct input]` badge; `leader+e` — also listed in the Ctrl+Space
  menu — turns it back on. Ctrl+Space inside the composer opens the
  leader menu directly.
- Unsent compose drafts (text and attached images) survive closing the
  box; the next keystroke in that session restores them. Submissions
  also clear any leftover text in Claude Code's input first, so stray
  typed characters can no longer merge into your prompt.

### Fixed

- The toast shown when the composer switches a session to direct input
  (after running an interactive command like `/model`, or via
  `leader+e`/Ctrl+E) was too long and got truncated, hiding the part
  that told you how to get back. It now clearly reads "Composer off —
  leader+e to re-enable" as a warning so the state change stands out.
- Claude Code panes could garble in the embedded view: the input box
  drifted up a row and bled its text into the divider above it, and a
  repaint (leader-R) only cleared it until the next update. The garble
  is in the real tmux grid — Claude Code's incremental renderer draws
  its input box at a stale anchor row and leaves the vacated cells
  behind — so AMF was faithfully showing corrupted pane content rather
  than mis-rendering. AMF now re-anchors a live Claude pane every few
  seconds with a one-row SIGWINCH bounce, forcing Claude Code to fully
  repaint and clear the stale cells. The bounce is hidden: the display
  holds its last good frame while the shrink/restore and repaint happen
  off-screen, so a clean pane shows no wobble and a garbled one just
  resolves in place. Other harnesses fully repaint on their own and are
  left untouched.
- The control-mode view worker also re-captures the full pane on a
  ~250ms self-heal floor instead of only on detected output, so any
  frame the change-notifier misses no longer lingers until the 3s drift
  reseed.

### Migration

- The compose input is on by default for Claude Code sessions, so
  typing in a Claude view now opens the AMF composer instead of going
  straight to Claude Code. If you prefer the old behavior for a
  session, press `leader+e` (or Ctrl+E inside the composer) to switch
  that session to direct input.

## [v0.22.0] - 2026-06-12

### Added

- The startup loading screen now shows short tips for easy-to-miss
  commands, including the view refresh shortcut for fixing visual
  glitches in embedded sessions.
- Leader → Shift-R in view mode now repaints the agent's screen on
  demand. If an agent's display ever desyncs mid-turn (text appearing
  on the wrong line while it streams — a Claude Code rendering bug,
  not an AMF one), one keystroke forces a full redraw instead of
  waiting for it to fix itself.
- Agent sidebars now show the active model when AMF can determine it
  for Claude, Codex, and OpenCode sessions, making it easier to confirm
  which model a running agent is using.

### Changed

- Pending input requests can now surface sooner when AMF starts in an
  embedded session view. The default input-request startup wait is now
  1.5 seconds, and you can tune it with
  `input_request_wait_seconds` in `~/.config/amf/config.json`.
- Diff-review prompts now only open automatically while you are viewing
  the feature that requested the review. From the dashboard or another
  feature's view, the review is added to the pending input requests and
  announced with a toast instead of stealing focus; open it from the
  input picker, by entering the feature view, or with `V`.

### Fixed

- The embedded view no longer corrupts permanently when something else
  resizes an agent's pane behind AMF's back (a second AMF instance,
  attaching the session directly in another terminal). AMF now
  notices the size drift within a few seconds and restores it, and it
  backs off instead of fighting if another instance keeps resizing.
- The embedded pane now fills the full view area (it was always two
  rows short), and typing echo lands on the correct row. A subtle
  capture quirk shifted the whole frame up one line whenever the
  screen was exactly full, leaving the cursor floating below the text.
- Typing into an agent no longer turns sluggish while it is streaming
  output. Screen updates from fast agent output are now paced so
  keystrokes keep priority, and keystroke echo is never delayed behind
  a backlog of redraws.
- Dragging or re-tiling the AMF window now resizes the agent's pane
  once, after the size settles, instead of once per animation frame.
  Repeated mid-stream resizes were the main way garbled "ghost" rows
  got baked into an agent's scrollback.
- AMF now detects when its tmux server was started by a different AMF
  build with a different bundled tmux version. Previously this
  mismatch silently broke the fast view path and session-event
  updates; both now fall back cleanly, with a clear note in the debug
  log.
- Session status events now arrive from the tmux server AMF actually
  uses. They were being watched on the wrong server, so status changes
  silently fell back to slow polling for everyone.
- Codex sidebar model details now remain visible after usage and
  activity lines appear by letting the Status section grow to fit its
  contents.
- The vibeless diff-review popup now appears on macOS. The hook's
  `amf notify-wait` call bound its reply socket under `$TMPDIR`, whose
  long per-user path on macOS exceeds the Unix socket path limit, so
  IPC delivery silently failed on every review. Reply sockets now live
  in the short state directory used by the main AMF socket.
- Fallback notification files (written when IPC delivery fails) now
  open the diff-review popup automatically instead of waiting for a
  manual `V` refresh. The existing filesystem watcher picks them up
  the moment they are written; no polling was added.
- The Terminal option is back in the new-session picker. It was
  accidentally dropped in v0.20.0 alongside an intentional change to
  the new-feature dialog, so adding a plain terminal to a running
  feature was impossible. The new-feature dialog is unchanged.

### Migration

- No migration is required. To override the new default, set
  `"input_request_wait_seconds": 1.5` in
  `~/.config/amf/config.json` and adjust the value as needed.

## [v0.21.0] - 2026-06-11

### Changed

- AMF now stays fast after running for hours or days. Usage and token
  statistics are computed in the background and re-read only what
  changed since the last check, so input no longer stalls as the day's
  agent transcripts grow.
- View mode now uses far less CPU while idle. The embedded pane is
  updated when output actually arrives instead of being re-captured
  many times per second, with no change to typing echo or streaming
  responsiveness.
- Hook and agent notifications now wake AMF immediately instead of
  waiting for the next poll tick, so toasts and pending-input alerts
  appear without delay.
- The debug log (`~/.local/state/amf/debug.log`) is now capped at
  10 MB with one rotated generation kept, so it no longer grows
  without bound. Routine per-message IPC chatter is summarized once
  per 5 seconds instead of logged line by line.
- The dashboard now reacts immediately when agent sessions start or
  stop: AMF listens for tmux session events instead of polling every
  5 seconds, so a feature that finishes or dies shows its new status
  right away.
- Agent activity indicators and sidebar prompts now update from
  filesystem events instead of timed scans. Thinking status appears
  as soon as an agent reports it, and an idle AMF does close to zero
  background work regardless of how many features exist.
- AMF now keeps a small hidden tmux session (`_amf-observer`) while
  running so it can receive those session events. It never appears in
  AMF's pickers and is removed when AMF exits.

### Fixed

- Fixed periodic input stalls in view mode caused by usage statistics
  being recalculated on the main thread while agents were streaming.
- An idle embedded pane no longer spawns background tmux processes
  several times per second.

### Migration

- No migration is required. The token-usage cache database is upgraded
  automatically on first launch.

## [v0.20.0] - 2026-06-03

### Added

- New feature creation now includes a session naming step before launch,
  prefilled with the default harness name such as `Claude 1` or
  `Codex 1`, so you can rename the initial agent session before it
  starts.
- Starting an additional session now asks for a session name after you
  choose the session type, with the current automatic name filled in by
  default.
- Existing-worktree feature creation now supports `/` search in the
  worktree picker, so large worktree lists can be filtered before
  selecting one.

### Changed

- The session picker no longer offers `Terminal` as a built-in session
  type.
- Feature creation now scopes feature session and worktree names by
  project, so separate projects can both use names like `main` or `tt`
  without attaching to the wrong session or colliding on the same
  worktree path.

### Fixed

- The new feature form now shows duplicate or invalid feature names
  inline on the `Name` field and lets you correct the name before
  continuing.
- Fixed slow typing in the new feature form by avoiding repeated config
  lookups while the dialog redraws.
- Fixed Vibeless diff reviews that could get stuck on Claude's file
  update step when Claude reported a working directory that did not
  match AMF's stored feature path. AMF now identifies the waiting
  review by its managed tmux session first, so the review dialog opens
  immediately instead of requiring the manual `V` recovery shortcut.
- AMF now skips broken Claude Code auto-update binaries when launching
  Claude sessions or headless Claude commands. If the newest installed
  Claude binary fails `--version`, AMF tries the next installed version
  before falling back to `claude` on `PATH`, so a bad Claude update no
  longer prevents AMF-managed Claude sessions from starting.

### Migration

- No migration is required.

## [v0.19.7] - 2026-05-14

### Changed

- Pending diff reviews now show up in `Work -> state` for Claude,
  Codex, and Opencode sessions, with a `leader V` hint when the
  review popup is not appearing.

### Fixed

- Pending Claude diff reviews now stay visible in the sidebar `Work`
  section while they are waiting, and clear from the sidebar after the
  review is submitted.
- Fixed vibeless diff-review not appearing when AMF was on the
  dashboard. Previously, a diff-review arriving while the dashboard was
  open would be silently queued and never shown — the review request
  would time out after 120 seconds and the agent would stall waiting
  for a response that never came. This was most noticeable on macOS.
  The review dialog now opens immediately regardless of which screen
  you are on.

### Migration

- No migration is required.

## [v0.19.6] - 2026-05-14

### Added

- Vibeless diff reviews now have a recovery shortcut, so you can check
  for a pending review from the dashboard or embedded view and open it
  manually when the normal popup flow gets stuck. This is available as
  `V` on the dashboard for feature/session rows and as `Ctrl+Space`
  then `V` while viewing a session.

### Changed

- Opencode sidebar updates now use AMF's IPC path when available and
  keep fallback file checks off the UI thread, so sidebar refreshes no
  longer risk making the dashboard or embedded view feel stuck. That
  keeps the view responsive while sidebar state is loading or updating.

### Migration

- No migration is required.

## [v0.19.5] - 2026-05-11

### Fixed

- Fixed the remaining sources of startup latency that persisted through
  v0.19.4. Three changes ship together:
  - **Prompt cache tail-read**: `read_prompts_from_claude_sessions` now
    reads only the last 64 KB of the most-recently-modified session file
    per feature instead of loading all `.jsonl` bytes across every session
    file. For features with a long Claude history this reduces prompt-cache
    time from seconds to microseconds.
  - **Token-count off the hot path**: the today-token calculation
    (`calculate_claude_today_tokens`) previously blocked the startup
    usage-refresh task by reading every `.jsonl` file modified today
    across all `~/.claude/projects/` subdirectories. It now runs in a
    dedicated background thread; the usage display updates once the count
    arrives without delaying the dashboard.
  - **Loading gate trimmed**: the session-status background thread
    (`session_status_bg`) no longer holds the "Loading AMF..." screen
    open. Token-usage counts are cosmetic; the dashboard now appears as
    soon as the other startup tasks finish and the counts fill in
    asynchronously.

### Migration

- No migration is required.

## [v0.19.4] - 2026-05-11

### Fixed

- Fixed the dominant cause of the "Loading AMF..." stall that persisted
  through v0.19.3. `App::new` was synchronously reading every Claude
  session `.jsonl` file (potentially megabytes per feature) and every
  `PLAN.md` to pre-populate the prompt and plan caches before the first
  frame could draw. Both caches now start empty and are filled by the
  background sidebar-load tasks that run immediately after the dashboard
  appears, so startup is fast regardless of session history size.

### Migration

- No migration is required.

## [v0.19.3] - 2026-05-11

### Fixed

- Fixed slow startup (stall on "Loading AMF..." or "Refreshing Claude
  hooks...") that became noticeable after the v0.19.0 global store
  migration. With a large feature count, `ensure_notify_scripts` and
  `ensure_amf_skills` were writing tens of files per feature on every
  launch. The hook and plugin refresh passes now record a version stamp
  after completing; subsequent startups on the same binary skip both
  passes entirely. Individual script and skill writes are also guarded
  by a content check so they are no-ops when already up to date.
- Eliminated a 50ms idle gap between each startup task; the event loop
  now spins without delay while startup tasks are pending, so the loading
  screen clears as fast as the tasks complete.

### Migration

- No migration is required.

## [v0.19.2] - 2026-05-11

### Fixed

- View mode now stays responsive while typing again, but still refreshes
  periodically when an agent harness is working, so live output keeps
  moving without waiting for the next keypress.
- Control-mode view input now uses the cheaper burst path again, which
  removes the extra redraw work that made repeated typing feel slower.
- Sidebar metadata and worktree sidebar updates now trigger redraws as
  soon as they arrive, so harness-side status changes appear without an
  extra keystroke.
- Fixed slow startup ("Loading AMF..." screen stall) introduced in v0.19.0
  by the global project store migration. `ensure_notify_scripts` and
  `ensure_amf_skills` now skip disk writes when the on-disk content is
  already up to date, so startup I/O scales to a few cheap reads per
  feature instead of tens of unconditional writes.

### Migration

- No migration is required.

## [v0.19.1] - 2026-05-09

### Fixed

- Release automation now waits for the same `cargo test --locked`
  preflight used by CI before it tags a version, so a failing test suite
  stops the release earlier instead of creating a broken release object.
- Fixed the view snapshot test harness so it no longer expects a return
  value from `send(...)` after the channel sender change.

## [v0.19.0] - 2026-05-09

### Added

- Debug log overlay now supports `p` to hide perf entries when you want
  to focus on non-performance messages.
- Feature create and delete actions now write richer audit entries to
  the debug log so a removed feature is easier to reconstruct later.
- View mode now shows a real scrollbar while you are in scroll/copy mode, so it is easier to tell that the pane is being scrolled instead of forwarded directly to tmux.
- Scroll/copy mode now supports fast movement with `Ctrl+j`, `Ctrl+k`, `Ctrl+Up`, and `Ctrl+Down`, matching the faster scrolling behavior used in other viewers.

### Changed

- The dashboard header now shows the AMF version next to the app title.
- AMF now uses one global project database at `~/.config/amf/amf.db`
  no matter which checkout you launch it from, so your project list stays
  consistent across directories.
- If you already had separate per-worktree project stores, AMF now merges
  them into the global database automatically the next time you start it.
  That means the first launch after upgrading may bring in projects from
  other checkouts instead of keeping them isolated.
- Scroll/copy mode now preserves the pane's ANSI coloring instead of flattening everything into plain text, so syntax highlighting and terminal colors remain visible while scrolling.
- The scroll-mode header now makes the active mode more explicit for users who are reading the status line.
- View mode now wakes immediately when new snapshots arrive, using a
  self-pipe wakeup and condvar-assisted worker polling to reduce input
  lag.
- GitHub releases now publish their notes from the matching changelog
  section, with a direct link back to the source entry in `CHANGELOG.md`.

### Migration

- No manual migration is required. AMF will fold legacy worktree-local
  project data into the global database on startup.

### Fixed

- Drag-to-copy selection now highlights correctly while you are in scroll/copy mode and still copies the selected text from the scrolled view.
- AMF now validates installed syntax highlighters at startup and repairs stale parser bundles automatically, so release builds should stop silently dropping syntax coloring.

## [v0.18.4] - 2026-05-08

### Fixed

- Eliminated the remaining source of input lag in view mode on all
  platforms, but most noticeably on macOS. The control-mode view worker
  was calling `reseed_control_view_parser` (two `tmux` subprocesses:
  `capture-pane` + `display-message`) on every keypress burst *and* on
  every control-protocol pane update, completely negating the benefit of
  having an event-driven control-mode view. The worker now:
  - On keypress burst: sends the current vt100 parser state immediately
    (zero subprocesses) so the display responds instantly; the actual
    pane update arrives shortly via the control protocol.
  - On control-protocol update (`parser_changed`): sends the
    incrementally-updated parser state directly (zero subprocesses)
    instead of re-capturing from tmux.
  - Periodic `NORMAL` reseeds and structural changes (pane swap, mode
    change, pause) still do a full reseed to correct any parser drift.

### Migration

- No migration is required.

## [v0.18.3] - 2026-05-08

### Fixed

- Fixed persistent "warning: could not set up terminal" on macOS even after
  the `xterm-256color` fix in v0.18.2. The root cause was that a user-set
  `TERMINFO` or `TERMINFO_DIRS` env var (e.g. a Homebrew ncurses path) was
  inherited by AMF's control-mode tmux clients, overriding the system
  terminfo lookup and preventing any terminal type — including `dumb` —
  from being found. AMF now strips `TERMINFO` and `TERMINFO_DIRS` from the
  environment of all spawned control-mode clients so they fall back to the
  compiled-in system terminfo paths where `xterm-256color` is reliably
  present.

### Migration

- No migration is required.

## [v0.18.2] - 2026-05-08

### Fixed

- Fixed noticeable input lag on macOS caused by the wrong terminal type
  (`screen-256color`) being used for AMF-managed tmux sessions. macOS's
  system terminfo does not include `screen-256color`, which caused the
  tmux control-mode clients to fail initialisation and fall back to
  spawning a `tmux send-keys` subprocess per keypress (~20–50 ms each).
  AMF now uses `xterm-256color` on macOS, which is present in the system
  terminfo, and explicitly overrides `TERM` when spawning control-mode
  clients so they are not affected by an inherited broken terminal type.
  This also eliminates the "warning: could not set up terminal" message
  that appeared when opening a terminal inside an AMF session on macOS.

### Migration

- No migration is required.

## [v0.18.1] - 2026-05-08

### Fixed

- Linux `amf upgrade` now skips unsupported packaged file types inside the
  bundled `tmux-root` tree, which prevents failures when copying release
  assets that contain special entries such as package docs.

### Migration

- No migration is required.

## [v0.18.0] - 2026-05-08

### Added

- Toast notifications now surface input requests and other transient
  prompts directly in the dashboard.
- AMF skills can now be injected into feature workspaces when a feature
  starts.
- Mouse-wheel scrolling now works in the Markdown viewer and help
  dialog.

### Changed

- Project storage, token caching, debug logging, and session status
  tracking now use SQLite-backed persistence.
- Codex notification hooks now use the updated `codex_config` flow, and
  Codex settings overrides are merged into the local workspace config.
- The tmux viewing stack now has a fallback path for environments where
  control-mode is unavailable.
- Startup now shows a loading screen while AMF initializes.
- Embedded overlays now keep the tmux cursor hidden behind dialogs and
  other UI surfaces.

### Fixed

- Embedded tmux view updates now reseed from tmux when control-mode
  output arrives, which prevents stale whitespace from lingering until
  the next manual input or view refresh.
- Toasts now render correctly in Viewing mode.
- macOS control-mode space rendering now works correctly.
- `amf upgrade` now handles symlinked release paths correctly.
- Harness setup can now be dismissed cleanly.

### Migration

- Existing stores migrate in place to the SQLite-backed schema; no
  manual migration is required.

## [v0.17.1] - 2026-04-21

### Fixed

- Managed tmux control-mode sessions now bootstrap with a temporary hidden
  session before applying the global `default-terminal` setting, avoiding the
  macOS startup failure where tmux could not connect to the managed socket.
- tmux startup on macOS now handles the dedicated managed socket without
  relying on `tmux start-server`, which could fail with `server exited
  unexpectedly`.

### Migration

- No store migration is required.

## [v0.17.1] - 2026-04-21

### Fixed

- macOS cross-compilation now skips the PTY termios setup that is not
  available on that target, resolving the build failure in
  `src/tmux.rs`.

### Migration

- No store migration is required.

## [v0.17.0] - 2026-04-20

### Added

- Embedded tmux sessions now use a full tmux control-mode view by
  default, streaming pane output directly into AMF for much more
  responsive typing and rendering in view mode.
- Added `tmux_control_mode` to `~/.config/amf/config.json`. It defaults
  to `true`; set it to `false` to return to the legacy ambient tmux
  socket and direct `tmux send-keys` fallback path.
- Help dialogs now support scrolling so longer keybinding and workflow
  reference text remains readable inside smaller terminals.

### Changed

- AMF now uses a dedicated managed tmux socket for control-mode sessions
  instead of inheriting a potentially polluted ambient tmux server.
- View-mode input no longer relies on per-key `tmux send-keys`
  subprocesses in the default path, reducing input latency and avoiding
  stale control-client buildup on long-running tmux servers.
- Diff-review prompts now include a short hold delay to avoid accidental
  keystrokes being interpreted immediately after the review popup opens.

### Fixed

- Control-mode view reseeding now restores the parser cursor to tmux's
  real pane cursor before applying incremental output, fixing misplaced
  cursor and stray text artifacts during shell/readline redraws.
- Session selection redraws now update correctly after switching
  sessions.
- Control-mode clients now perform readiness checks and fall back safely
  if startup fails.

### Migration

- No store migration is required.
- Existing tmux sessions on the previous ambient socket are not moved to
  the new managed control-mode socket. Restart those sessions from AMF,
  or temporarily set `"tmux_control_mode": false` in
  `~/.config/amf/config.json` if you need to keep using the legacy tmux
  server.

## [v0.16.0] - 2026-04-20

### Added

- Claude and Opencode sidebars now show task/todo progress with a
  compact progress bar, checkbox-style status markers, and a focused
  window around active work.
- Debug log navigation now supports `PageUp`/`PageDown`, `g`/`G` for
  top/bottom jumps, mouse wheel scrolling, and an explicit end-of-log
  marker.

### Changed

- Startup session-status sync now runs in the background instead of
  blocking the main event loop, improving first-open responsiveness for
  large session histories.
- VS Code availability detection now runs asynchronously during startup
  rather than blocking `App::new()`.
- The sidebar prompt section is more compact: the `leader l` hint moved
  into the border title, prompt text renders directly without a
  `Preview:` prefix, and prompt copy uses the primary text color.
- Persistent tmux control-mode input is now guarded behind
  `AMF_EXPERIMENTAL_PERSISTENT_TMUX_INPUT`, with direct `send-keys`
  remaining the default path.

### Fixed

- macOS key release events from crossterm are now ignored at top-level
  key dispatch, preventing actions from firing twice for a single
  keystroke.
- Recursive markdown, slash-command, usage, and session metadata scans
  no longer follow symlinked directories, avoiding UI stalls caused by
  symlink cycles or unexpectedly large linked trees.
- tmux control-mode input fallback now waits for client readiness,
  detects dead persistent clients, respawns them when needed, and falls
  back to direct `send-keys` on failure.

### Migration

- No store migration is required.

## [v0.15.0] - 2026-04-13

### Added

- Agent harness configuration and setup flow. AMF now lets you choose
  which harnesses are enabled, persists that selection in
  `projects.json`, and can prompt for setup on startup when no
  harnesses are configured.
- Pi support as a fourth harness/session type alongside Claude,
  Opencode, and Codex.

### Changed

- UI language now refers to user-selectable agents as "harnesses" in
  dialogs, help text, and picker flows.
- Feature creation can now skip the default terminal session and skip
  steering prompt setup when those extras are not needed.
- Feature creation, session pickers, and related config flows now only
  show harnesses that are currently enabled.
- Dashboard activity indicators are now animated, making background
  work and harness checks easier to spot.

### Fixed

- `amf upgrade` now streams release downloads to disk instead of
  buffering the full archive in memory first, improving reliability for
  larger bundles and lower-memory systems.
- Diff syntax highlighting now refreshes its cache correctly, reducing
  stale or incorrect highlighting in the diff viewer. Added multi-file
  syntax fixtures to make regressions easier to catch.

### Migration

- Existing stores migrate in place to keep using project store version
  5 while adding the new `available_harnesses` field.
- After upgrading, AMF may ask you to configure at least one harness
  before feature creation or session picker flows are available.

## [v0.14.1] - 2026-04-07

### Changed

- Dashboard status syncing now scales better with large project lists by
  using cached sidebar state for Opencode thinking detection and by
  reducing repeated visible-item and tmux-session scans.

### Fixed

- Embedded dashboard performance no longer degrades as sharply on
  machines with many projects, features, and open tmux panes due to
  repeated background `tmux capture-pane` fallbacks and redundant
  session-list work.

## [v0.14.0] - 2026-04-03

### Added

- Embedded view now supports `Ctrl+Space` then `R` to refresh tmux pane
  sizing on demand after terminal or layout changes.

### Changed

- Linked git worktrees now keep branch-local AMF state in
  `.amf/projects.json`, seeded from the primary checkout on first
  launch, so project and feature changes in one checkout no longer leak
  into another.
- Embedded tmux view refresh was reworked for better responsiveness,
  reducing idle overhead and making pane updates feel faster while you
  type, submit prompts, and interact with sessions.

### Migration

- No manual migration is required.
- The primary checkout still uses `~/.config/amf/projects.json`.
- The first AMF launch inside a linked worktree creates a local
  `.amf/projects.json`, initialized from the primary store when one
  exists.

## [v0.13.1] - 2026-03-31

### Fixed

- `amf upgrade` now replaces bundled release directories recursively,
  preventing partial installs that could leave the tmux wrapper present
  without its neighboring `tmux-real` binary or bundled support files.

## [v0.13.0] - 2026-03-26

### Added

- Opencode sidebar with work section, todos list, and sidecar state
  tracking — shows task activity, todo items, and LSP metadata
  alongside other session details.
- Per-session Codex prompt history and preview in sidebar — prompts are
  now session-specific rather than shared across features.
- Codex sidebar session metadata display including thread information,
  usage source confidence, and reasoning token counts.
- Local command actions in command picker — focused access to AMF-level
  actions without mixing in session-specific commands.
- Claude session resume picker on `S` now works for Claude sessions as
  well as Opencode, with session titles pulled from the first user
  prompt in each saved conversation.
- The steering prompt coach now supports scrolling for longer prompts
  without leaving the feature-creation flow.

### Changed

- Sidebar layout refinements across Codex and Opencode sessions for
  improved visual hierarchy and compactness.
- Codex sidebar summary and prompt sections reorganized to prioritize
  active work and plan context.
- Session pickers now show cleaner titles and relative ages for saved
  Claude, Codex, and Opencode sessions.
- Sidebar background refresh work now pauses while the sidebar is
  hidden, reducing unnecessary polling and improving view responsiveness.
- Sidebar, token usage, and usage refresh paths were reworked for lower
  overhead background updates.

### Fixed

- Stale worktree delete failures are now handled gracefully without
  blocking feature deletion.
- tmux session launches no longer leak AMF-managed `PATH` overrides into
  child sessions.
- `amf upgrade` now resolves the actual release asset from GitHub's
  release metadata instead of hardcoding a guessed download URL, so
  future packaging changes do not regress into `404` download failures.
- macOS `x86_64` upgrade detection now only selects the Apple Silicon
  bundle when AMF is running under Rosetta on Apple Silicon. Native
  Intel Macs now get a clear unsupported-platform error instead of a
  misleading architecture mapping.

### Migration

- No store migration is required.

## [v0.12.0] - 2026-03-24

### Added

- Claude session sidebar — a new panel in view mode showing live session
  metadata: current tool activity, pending input detail, active prompt
  context, task todos (expanded inline), and plan progress. Toggle
  visibility with a keybind. Task data is sourced from the Claude task
  store when available, with transcript fallback.
- Latest prompt dialog now shows a scrollable list of all Claude session
  prompts with timestamps. Navigate with `j`/`k`, copy the selected
  prompt to clipboard with `y` (uses `wl-copy` with `xclip` fallback).
  Each entry shows a colored timestamp and the first line of the prompt,
  truncated with an ellipsis when needed.

### Changed

- Markdown viewer and picker UX improvements.

### Migration

- No store migration is required.

## [v0.11.1] - 2026-03-19

### Changed

- Improved TSX syntax highlighting in the diff viewer.

### Fixed

- Restored sessions now resize correctly to the current pane dimensions
  on attach, and the session picker no longer wraps unexpectedly on
  narrow terminals.
- `amf upgrade` now downloads the full `.tar.gz` bundle and extracts
  all bundled files (`amf`, `tmux`, `tmux-real`, libs) into the install
  directory, so the bundled tmux binary is also updated on upgrade.
- Install and upgrade scripts remove the existing `/opt/amf` directory
  before moving the new bundle into place, preventing the old binary
  from being left behind when `/opt/amf` already exists.

## [v0.11.0] - 2026-03-17

### Added

- Per-session token usage tracking — Claude, Codex, and Opencode agent
  sessions now show a live cost line in the dashboard:
  `2.3M in · 4.8k out · 304.8k eff · $0.91`. Pricing defaults to
  Claude Sonnet 4.x rates and is configurable via `token_pricing` in
  `config.json`. Set `show_cost: false` to hide the dollar column.
- Steering prompt editor — edit the feature's steering prompt directly
  from the dashboard without leaving the TUI. Accessible via the
  feature creation flow and a new view-mode shortcut.
- Gruvbox Dark and Gruvbox Light UI themes, plus a matching
  `amf-gruvbox.json` Opencode theme with full syntax, markdown, and
  diff highlighting.
- Live theme preview in the theme picker — scrolling through themes
  applies them immediately; `Esc` reverts to the original and `Enter`
  confirms. Press `t` inside the picker to toggle transparent
  background on the fly.
- Latest prompt injection — press `Tab` or `Enter` in the latest
  prompt dialog (leader `l`) to send the saved prompt directly into
  the active session without leaving view mode.

### Changed

- Memo sessions removed — the `m` keybind, `has_notes` field, and
  all related UI and automation API surface have been dropped. Existing
  features with notes are unaffected at the data level, but the session
  type will no longer appear in pickers.
- Session picker no longer spawns a `code --version` subprocess on
  every open; VSCode availability is cached at startup. Config is also
  read from the already-loaded extension instead of hitting disk again.

### Fixed

- Thinking/tool hook scripts (`thinking-start.sh`, `tool-start.sh`,
  etc.) now use `$AMF_SESSION` (the tmux session name) as the IPC
  key instead of the Claude hook UUID. This fixes the thinking
  throbber never appearing in IPC mode.
- Bundled `ld-linux` is used when invoking the bundled tmux on
  systems where the host glibc version is too old, preventing
  "version not found" errors on older Linux distros.
- UI hangups caused by blocking file I/O in the usage refresh path
  are eliminated.

### Migration

- No store migration is required.
- If you relied on Memo sessions, those session entries will no longer
  start or appear in pickers. Remove them from saved features if
  desired.
- If you have custom `token_pricing` needs, add a `token_pricing`
  block to `~/.config/amf/config.json` (see configuration docs).

## [v0.10.1] - 2026-03-13

### Fixed

- `custom-diff-review.sh` now resolves `NOTIFY_DIR` from the git
  repository root rather than the current working directory. This
  fixes missed notifications when Claude operates in a subdirectory
  of the repo.

## [v0.10.0] - 2026-03-12

### Added

- AMF release bundles now include a statically-linked `tmux` binary.
  When launched outside an existing tmux session, AMF uses the bundled
  binary on a private socket so that tmux does not need to be installed
  separately.
- `AMF_TMUX_BIN` and `AMF_TMUX_SOCKET` environment variables let you
  override the tmux binary and socket path at runtime.

### Changed

- Default branch name changed from `master` to `main` throughout —
  diff review scripts, PR helpers, and worktree operations now default
  to `main` as the base branch.

### Migration

- No store migration is required.
- If you have existing scripts that relied on `master` as the default
  base branch, update them to use `main` (or set the branch explicitly).

## [v0.9.0] - 2026-03-12

### Added

- On-demand tree-sitter parser management — a language picker lets you
  install and select syntax highlighting grammars at runtime without
  restarting, accessible from the diff viewer and diff review prompt.
- Scroll support in the diff review prompt pane (j/k, g/G, mouse wheel).
- Opencode change-tracker plugin (`.opencode/plugins/change-tracker.js`)
  that watches file writes, emits AMF notifications, and wires into the
  diff review approval flow for Opencode sessions.

### Fixed

- Diff review flow for Opencode sessions now correctly triggers the
  change-reason prompt and handles accept/reject signalling.
- Diff review patch scroll state is now shared consistently between the
  diff viewer and diff review prompt.

### Migration

- No store migration is required.
- To use Opencode diff review, the
  `.opencode/plugins/change-tracker.js` plugin must be present in your
  repo (included automatically for new features).

## [v0.8.0] - 2026-03-11

### Added

- Built-in in-app diff viewer for view mode with a file list, patch pane,
  unified and side-by-side layouts, refresh support, and keyboard
  navigation.
- Tree-sitter syntax highlighting for the diff viewer, plus contextual
  line highlighting and clearer diff gutters.
- In-app markdown file picker and viewer for `.claude/*.md` files and
  top-level `*.md` files while viewing a feature.
- Repo-root markdown discovery for worktree features, so shared files
  like `PLAN.md` can be opened without leaving the current session.
- Vibeless-mode Codex diff review automation that watches file writes,
  opens the change-reason prompt, and reverts rejected changes.

### Changed

- Diff review and markdown-reading workflows now stay inside AMF instead
  of requiring an external tool or a separate terminal flow.
- Markdown picker labels now distinguish worktree files from repo-root
  files when both scopes are available.

### Migration

- No store migration is required.
- If you use Codex vibeless-mode diff review, install `inotifywait`
  (usually provided by `inotify-tools`) so the watcher can run.

## [v0.7.0] - 2026-03-09

### Added

- Full automation system for creating projects, features, and batch features via CLI and IPC
  - `amf automation create-project` for programmatic project creation
  - `amf automation create-feature` for programmatic feature creation
  - `amf automation create-batch-features` for parallel multi-feature creation
  - JSON-based request/response interface with timeout and dry-run support
- Steering Coach startup prompt overlay for coaching new features
- Plan mode for collaborative planning sessions with shared PLAN.md
- Show pending worktree scripts in project list with visual indicators
- Project preferred agents configuration per workspace
- Worktree session configuration dialog for customizing sessions
- Mouse wheel support for pane scrolling in view mode
- Codex session restore functionality
- Release session now displays current version before prompting for new version

### Changed

- Release session moved from global to local repo configuration
- Better error handling and status messaging throughout the application
- Improved review mode selection and behavior
- Enhanced Codex thinking detection for repo-root features
- Fixed Codex latest prompt lookup
- Worktree script visibility improved with blocking operations

### Fixed

- Review mode selection now correctly handles feature states
- Codex thinking detection properly works for features using repo root directly
- Session restore functionality works across different agent types

### Migration

- No manual migration required, but review the new automation interface if integrating AMF into CI/CD workflows

## [v0.6.1] - 2026-03-06

### Fixed

- Fixed extension reload path handling for workspace-local
  `.amf/config.json`.
- Repaired related test fixtures around extension loading.

### Migration

- No manual migration required.

## [v0.6.0] - 2026-03-06

### Added

- Full AMF UI theming with built-in `default`, `amf`, `dracula`,
  `nord`, and Catppuccin variants.
- A dashboard theme picker and `theme` / `transparent_background`
  config support.
- `allowed_agents` config so each workspace can restrict AMF to a
  subset of Claude, Codex, and Opencode.
- Session bookmarks with `H`, `M`, and `1`-`9` quick
  jumps.
- Ready-state tracking for features.
- Configurable leader timeout via `leader_timeout_seconds`.
- Codex usage bars in the status area plus extra usage debug logging.

### Changed

- Leader mode now opens clearer popup menus in view mode.
- Codex notifications prefer IPC delivery and merge with local
  worktree configuration.
- Debug log rendering wraps long lines instead of clipping them.

### Migration

- Optional: add `theme`, `transparent_background`,
  `leader_timeout_seconds`, or `extension.allowed_agents` to
  `~/.config/amf/config.json` or a repo-local `.amf/config.json`.
- No store migration is needed. `projects.json` is auto-migrated on
  load.

## [v0.5.0] - 2026-03-06

### Added

- Initial Codex agent and session support.
- IPC-based input notifications with file-based fallback when the AMF
  socket is unavailable.
- Improved input request handling for Codex worktrees.

### Changed

- Session picker UX was tightened up around mixed agent/session types.

### Migration

- Install the `codex` CLI before creating Codex-backed features.
- No manual migration is required for existing Claude or Opencode
  features.

## [v0.4.1] - 2026-03-05

### Added

- `amf upgrade` to replace the installed binary with the latest GitHub
  release.
- `amf -V` / `amf --version` for quick version checks.

### Migration

- No manual migration required.

## [v0.4.0] - 2026-03-04

### Added

- Batch feature creation for spinning up numbered worktrees in one
  flow.
- Feature nicknames shown in the dashboard.
- Per-feature/session workdir handling for local extension config.

### Changed

- Forking preserves uncommitted changes when creating the new feature.
- Claude thinking detection became mtime-based for lower overhead and
  better responsiveness.

### Migration

- Repo-local `.amf/config.json` files are now respected alongside the
  global config and merged on top of it.
- Existing saved state is auto-migrated to include summary and
  nickname fields.

## [v0.3.0] - 2026-03-04

### Added

- Custom session `autolaunch`, `on_stop`, status text, and `pre_check`
  support.
- Session forking with transcript context export.
- Theme picker dialog in the dashboard.
- Auto-generated session summaries.
- Mouse text selection and clipboard copy in the embedded pane.
- Debug log overlay with file logging.
- Claude session resume picker on `S`.

### Changed

- Removed the old "switch directly to tmux" workflow in favor of the
  embedded view.
- Final review no longer kicks you out of viewing mode when there are
  no changes.

### Migration

- If you use custom sessions, you can now optionally add `autolaunch`
  and `pre_check` fields to their config entries.
- No manual store migration is required.

## [v0.2.0] - 2026-03-02

### Added

- Built-in AMF themes, transparent background support, and bundled
  Opencode themes for the embedded pane.
- VSCode session support launched through the `code` CLI.
- Saved latest Claude prompt overlay.
- Custom session status relays and `on_stop` cleanup hooks.

### Migration

- Optional: set `theme`, `transparent_background`, or
  `opencode_theme` in `~/.config/amf/config.json`.
- If you use VSCode sessions, make sure the `code` CLI is installed.

## [v0.1.1] - 2026-03-01

### Added

- Initial public release of the multi-project, multi-feature dashboard.
- Embedded tmux view for Claude and Opencode sessions.
- Git worktree orchestration, vibe modes, notifications, search, and
  session management.

### Migration

- First tagged release. No migration required.
