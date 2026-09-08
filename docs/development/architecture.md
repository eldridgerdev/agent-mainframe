# Architecture and refactor map

This describes revision `c4088b70cd22393dd055dd249816da6e0f6825f0`, before
maintainability refactors. See [baseline evidence](baseline/README.md) for the
complete test inventory, planned test paths, helper callers, and App fields.
The M2 test layout below is now implemented; production paths still describe
the baseline unless explicitly labelled as destinations.

## Runtime and dependencies

`src/main.rs` owns CLI dispatch, startup, terminal setup/teardown and `run_loop`.
The loop reads terminal/IPC events, polls feature workers, reconciles external
session state, schedules pane refreshes, and redraws through `ui::draw`. It uses
deadlines and mode-dependent work, not one fixed polling interval. Viewing and
Compose both keep the pane live; active PR sweeps have an independent cadence.

```text
main (startup, events, deadlines, worker polling)
  -> handlers (key/mouse dispatch) -> app feature methods
  -> ui (dashboard, pane, pickers, dialogs) -> App/state/display helpers
app feature methods -> project/domain data + feature state
                    -> db::AmfDb + feature persistence modules
                    -> traits::{TmuxOps, WorktreeOps} / external managers
workers -> channels/mailbox -> App poll/apply methods -> AppMode/display state
```

`src/handlers/mod.rs` routes by `AppMode`; specialized handlers translate input
into feature operations. `src/ui/dashboard.rs` dispatches the renderer and
`src/ui/dialogs/` renders overlays. Neither layer should acquire new persistence
or process-launch responsibilities during extraction. Existing handlers also
mutate dialog fields directly: the diagram is a responsibility map, not a claim
that App already exposes a fully encapsulated API.

`src/app/mod.rs` declares feature modules, re-exports state, defines App/AppConfig,
constructs production and test instances (`new`, `new_for_test`), and owns shared
pane/sidebar worker infrastructure, logging, and configuration access.
`src/app/state.rs` holds `AppMode`, selection and most dialog state, including
feature-specific receivers and child-process guards. Feature files are mostly
`impl App` blocks; they can currently access unrelated App fields through their
parent module. File extraction alone will not remove that coupling.

`src/project.rs` models projects, features and sessions. `src/db/mod.rs` owns the
SQLite connection and open/seed entrypoints; `migrations.rs` owns schema changes.
`store.rs` persists project state. Other DB modules persist session status,
tokens, debug logs, editors, prompts/overrides, TODOs, plan interviews, Learning,
PR triage/investigations/terminal state, and PR/AI review caches. Feature code
calls these methods; the DB module also knows project/domain types and uses
WorktreeManager while resolving legacy stores. It is not an independent generic
storage layer.

`src/traits.rs` provides mockable TmuxOps and WorktreeOps, implemented by
`src/tmux.rs` and `src/worktree.rs`. GitHub operations live in `src/github.rs`;
headless agent execution in `src/headless.rs`; harness integration includes
Claude, Codex, OpenCode (under app), and Pi. IPC, filesystem watching, tmux
observation, editor tracking and resource inspection have dedicated modules.
These boundaries include filesystem writes, subprocess creation and OS handles.
Injection is partial: feature modules still call concrete managers and filesystem
APIs directly. Preserve those calls during mechanical moves; introduce narrow
adapters only in the subsequent ownership change.

## Test suites (M2 complete)

[app-tests.tsv](baseline/app-tests.tsv) maps every current `app::tests` test to
its implemented full path. The original function name must remain unchanged.
[tests.txt](baseline/tests.txt) also records tests outside that file, which stay
in place unless a later feature extraction warrants a move.

| Child module under `src/app/tests/` | Responsibility / production callers exercised |
| --- | --- |
| `startup_navigation` | Startup, utility naming, dashboard selection/navigation |
| `pane_input` | Pane mailbox/render updates, view input, Compose, command dispatch |
| `status_sidebar` | Session sync, IPC status, harness/sidebar caches and PR badges |
| `prompts_configuration` | AppConfig, context settings, prompt library/overrides, pre-call UI |
| `hooks_setup` | Local hook installation/cleanup and worktree hook completion |
| `feature_sessions` | Project/feature creation, recovery, start/stop, session pickers |
| `plans` | Plan interview, drafts, consent, synthesis and handoff |
| `automation` | CLI/automation project and feature operations |
| `pr_triage` | PR loading, AI review, investigations, replies, memory and integration |
| `final_review` | Diff review progression, comments, interdiff, tree/context controls |
| `todos` | TODO scopes, editing, launch and deletion disposition |
| `resources` | Agent admission and start/autostart guards |
| `editor_lifecycle` | Owned editor processes, PID identity and stop races |
| `attention` | Attention reconciliation, pending inputs and dashboard badges |

Learning's substantial existing suite is inline in `app/learning.rs`; there is
no Learning suite in the central file to move for M2. Retain it until M3.

[test-helpers.tsv](baseline/test-helpers.tsv) inventories all top-level helper
functions, the pending-interview constant, and BusyPane guard, with lexical
callers. Multi-suite helpers belong in `support.rs`; single-suite fixtures stay
local. Keep `App::new_for_test` on App initially, then share production/group
defaults in M4. Mock types remain generated from `traits.rs`. Preserve returned
TempDir/NamedTempFile lifetimes, per-test DBs, mock expectations and BusyPane's
Drop cleanup; do not create global fixtures or broaden production visibility.
Helper imports should use narrow test-only visibility. Shared fixture ownership
does not imply that a PR-specific builder is a general production abstraction.

## Feature extraction destinations (M3)

Do these sequentially, validating each feature before proceeding. Keep existing
entrypoints and temporary re-exports stable for main, handlers, UI and tests.

| Current owner | Destination children | Callers and boundary |
| --- | --- | --- |
| `app/pr_review.rs` | `pr_review/{mod,domain,fetch,actions,reply,investigation,integration,memory,state}.rs` | main polls; PR handlers/renderers, AI review, triage_feature and sync use its types/methods. Separate fetch/cache from decisions and reply/investigation work; coordinate existing triage_feature integration without duplicating it. |
| `app/review.rs` | `review/{mod,preparation,progression,comments,headless,state}.rs` | diff/review handlers and UI, feature launch, notifications, main polling. Preparation loads diff/history; progression selects/approves; comments handles anchors/editor/suggestions; headless coordinates walkthrough/co-review/overview/checks. |
| `app/learning.rs` | `learning/{mod,lifecycle,navigation,workers,follow_up,state}.rs` | Learning handlers/renderers, main answer polling and cross-feature TODO/plan handoff. Lifecycle opens/closes/persists; navigation lists/loads/selects; workers ask/apply answers; follow_up owns downstream workflows. |

Move PrReview/AI/reply/investigation/memory dialog types to the PR owner, review
viewer/comment/progression types to the review owner, and Learning types to the
Learning owner. Leave shared DiffScope/layout concepts available to ordinary
diff browsing, and keep `AppMode` central. AI review, review_destination,
review_memory and triage_feature already exist as neighboring modules: preserve
their responsibilities rather than absorbing them merely to shrink a file.
Pure transformations take explicit data; App orchestration keeps mode changes.
Feature modules may depend on shared domain/managers and narrow neighboring
entrypoints; do not import handlers/renderers into extracted domain modules.

## App ownership (M4)

[app-fields.tsv](baseline/app-fields.tsv) assigns all 130 App fields, including
private fields/channels, to feature or infrastructure ownership. Types are
recorded verbatim apart from whitespace. Referencing files are lexical navigation
aids (including tests and same-name tokens), not a semantic call graph.

Start with PR triage: `pr_review_bg`, `pr_investigation_bg`, `pr_review_return`,
review-memory workers/pending compaction, and AI review worker/progress/pending/
return/refresh state. Respect the existing distinction between a pending run and
an AppMode-owned display. The fix-cost and GitHub-user caches are feature-owned;
active/terminal PR badge caches and rate-limit backoff remain shared sync state.
Preserve successor invalidation across those owners.

Final Review's `awaiting_review_fixes` is on App, but much of its actual worker
and child ownership is already in `DiffViewerState` and related mode state.
Inspect that state before creating any new App group. Learning's App-level
`learning_answer_tx`, `learning_answer_rx`, and `learning_runs_in_flight` form a
coherent coordination group; displayed session data remains mode-owned. Preserve
session/request matching and abandoned-result handling on close/switch/reopen.

Other assigned owners (plans, TODOs, Compose/pane, context/usage, attention,
editors, hooks, summary and pre-call) identify responsibilities, not authorization
to refactor all of them in M4. Keep shared routing/config/store, injected managers,
IPC/watcher/observer guards, harness checks, debug/perf and viewport infrastructure
on App unless a concrete later boundary requires a move. Never replace App with
a generic context carrying the same unrelated state.

## Deferred work

The audit's store overwrite, migration atomicity, repository cache identity, IPC
socket ownership, startup resource handling and synchronous GitHub calls remain
separate reliability work. M1 changes no production code or persistence format.
No extraction-blocking dependency was found during this map; each M3 extraction
must still inspect its exact lifecycle and staleness guards before moving code.
