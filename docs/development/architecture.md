# Architecture

AMF is a single Rust executable. It retains the existing event loop and persistence
formats; feature extraction does not introduce a new framework or library.
See [checks.md](checks.md) for setup, focused suites, CI and contribution guidance.
The [baseline inventory](baseline/README.md) records the pre-refactor ownership map
and complete test preservation evidence.

## Runtime and dependency directions

```text
main (startup, events, deadlines, worker polling)
  -> handlers (key/mouse dispatch) -> App feature methods
  -> ui (dashboard, pane, pickers, dialogs) -> display state/helpers
App feature methods -> project/domain data + feature state
                    -> db::AmfDb + feature persistence modules
                    -> external managers / TmuxOps / WorktreeOps
workers -> channels/mailbox -> feature poll/apply methods -> AppMode/display state
```

`src/main.rs` owns CLI dispatch, terminal setup/teardown and `run_loop`. The loop
reads terminal/IPC events, polls workers, reconciles session state, schedules
pane refreshes and redraws. It uses deadlines and mode-dependent work; Viewing
and Compose both keep the pane live. Active PR sweeps have their own cadence.

`src/handlers/mod.rs` routes by AppMode. Handlers translate input into feature
operations and sometimes edit dialog fields directly. `src/ui/dashboard.rs`
dispatches renderers; `ui/dialogs/` renders overlays. Neither layer should gain
persistence or process-launch responsibilities.

`src/app/mod.rs` constructs App/AppConfig, declares features and owns shared
pane/sidebar infrastructure, logging and configuration access. Features remain
`impl App` orchestration. `src/app/state.rs` keeps AppMode and shared routing,
configuration, session and TODO/plan state, with compatibility re-exports of the
three extracted features' types. Cross-feature mode transitions remain on App.

## Feature modules

| Feature | Files under `src/app/` | Responsibility |
| --- | --- | --- |
| PR Triage | `pr_review/domain.rs` | Comment/review model, attribution, prompt/hunk transforms |
| | `pr_review/fetch.rs` | GitHub normalization, fetch/cache and PR picker coordination |
| | `pr_review/actions.rs` | Selection, marks and action orchestration |
| | `pr_review/reply.rs` | Reply drafting, provenance and posting |
| | `pr_review/investigation.rs` | Read-only investigation requests, results and follow-ups |
| | `pr_review/integration.rs` | Fix target/harness selection, injection and linked session navigation |
| | `pr_review/memory.rs` | Review-memory bootstrap, compact and append workflows |
| | `pr_review/state.rs`, `runtime.rs` | Dialog types and background work ownership |
| Final Review | `review/preparation.rs` | Diff snapshots, persisted progression/history and review notes |
| | `review/progression.rs` | Navigation, selection, approvals, filters and review summary |
| | `review/comments.rs` | Anchors, comments, suggestions and editor operations |
| | `review/headless.rs` | Walkthrough/co-review/check workers, completion and feedback dispatch |
| | `review/state.rs` | Viewer/comment/undo/history state and mode-owned child handles |
| Learning | `learning/lifecycle.rs` | Open/close, reload, settings and persistence coordination |
| | `learning/navigation.rs` | File trees, anchors, selection and answer navigation |
| | `learning/workers.rs` | Question prompts, execution and answer application |
| | `learning/follow_up.rs` | TODO creation and agent-session handoffs |
| | `learning/state.rs`, `runtime.rs` | Session/display types and persistent answer tracking |

Feature roots expose consumed entrypoints and compatibility re-exports. Free
helpers are imported from their owning sibling module; new private helper access
is bounded to the original feature. Extracted modules import explicit App types.
Pure transformations take data rather than an all-purpose App/context object.
State/domain code does not import handlers or renderers. Orchestration may use
existing cross-feature APIs for TODO/plan/session handoffs.

The neighboring `ai_review.rs`, `review_destination.rs`, `review_memory.rs` and
`triage_feature.rs` retain their existing responsibilities. Extraction does not
absorb them just to shrink a file. Shared DiffScope/layout concepts remain
available to ordinary diff browsing rather than becoming private to Final Review.

## Runtime ownership and stale results

App now has 125 fields, down from the 130 recorded in
[app-fields.tsv](baseline/app-fields.tsv). Three groups establish concrete
boundaries with private fields and shared production/test defaults:

- `PrReviewWork`: independent fetch/investigation receivers; begin, poll and
  cancel methods. Cancel drops the receiver without killing the spawned worker.
  AppMode still determines whether a result has a live target.
- `AiReviewRun`: receiver, pending origin and live progress. Completion or PR
  invalidation clears them together; closing the running screen preserves them
  for later result application and reopening. Existing PR/workdir/head matching
  stays in feature orchestration, including successor invalidation.
- `LearningRuns`: persistent answer channel and in-flight question IDs. Delivery
  retires its matching ID, then orchestration updates the visible matching row
  or the original DB row when the overlay has closed/switched. Closing the
  overlay does not cancel the worker or erase its pending identity.

Final Review already owns its children in mode state; it keeps that boundary.
No second authoritative copy was added. Shared routing/store/configuration,
manager injection, IPC/watcher/observer guards, pane/sidebar queues and debug/perf
remain on App. Review-memory jobs, refresh caches and cross-feature notification
state remain available for later incremental ownership work.

## Data and side effects

`src/project.rs` models projects, features and sessions. `db/mod.rs` owns the
SQLite connection and open/seed entrypoints; `migrations.rs` owns schema changes;
`store.rs` persists projects. Other DB modules persist session status, tokens,
debug logs, editors, prompt templates/overrides, TODOs, plan interviews, Learning,
PR triage/investigations/terminal state and PR/AI review caches. DB code knows
project/domain types and uses WorktreeManager to resolve legacy stores.

`src/traits.rs` provides mockable TmuxOps and WorktreeOps, implemented by
`tmux.rs` and `worktree.rs`. GitHub calls live in `github.rs`; headless execution
in `headless.rs`; harness integration supports Claude Code, Codex, OpenCode and
Pi. IPC, filesystem watching, tmux observation, editors and resource inspection
have dedicated modules. Injection is partial: App workflows still call concrete
managers and filesystem APIs. Preserve those boundaries and process lifetimes;
introduce an adapter only where it establishes a concrete dependency boundary.

## Tests

`src/app/tests/mod.rs` declares behavior suites and shared `support.rs`:

| Suite | Coverage |
| --- | --- |
| `startup_navigation`, `pane_input` | Startup, dashboard navigation, view/mailbox/input and Compose |
| `status_sidebar`, `attention` | Sync, IPC status, sidebar caches, badges and attention reconciliation |
| `feature_sessions`, `hooks_setup`, `automation` | Creation/recovery/start/stop, local hooks and automation entrypoints |
| `prompts_configuration`, `plans`, `todos` | Configuration/prompts, plan interviews and scoped TODO workflows |
| `pr_triage`, `final_review` | PR/AI/investigation/memory/integration and diff review workflows |
| `resources`, `editor_lifecycle` | Admission, process identity and editor stop races |

Each extracted feature also retains its own `tests.rs`; Learning's suite was
already local and its handler fixture paths remain stable. Four branch-matching
tests moved with PR state. Preserve the old/new path maps when auditing coverage.
Shared helpers belong in support only when multiple suites consume them. Temporary
repositories, DBs and process guards remain per-test, with their original lifetimes.

## Deferred reliability work

Store overwrite, migration atomicity, repository cache identity, IPC socket
ownership, startup resource handling and synchronous GitHub-call findings remain
separate reliability work. This refactor changes no persistence schema or user
workflow to address them. No extraction blocker was found; inspect exact identity
and cleanup guards before any future ownership change.
