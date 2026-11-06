# Code organization audit for AI context efficiency

Investigated 2026-11-04 at `be44e1ad`. Priority order: AI context efficiency,
readability, reuse, then separation of concerns. Findings below describe the
investigative baseline before implementation. Subsequent work is recorded at
the end of this report.

## Assessment

The existing architecture is usable and worth extending. Feature-owned modules
under `app/pr_review`, `app/review`, and `app/learning`, persistence modules under
`db`, and the shared `highlight` service provide concrete ownership boundaries.
The main opportunities are concentrated in a few central files and in navigation
between interfaces. A wholesale architecture rewrite would add risk and context
cost before producing useful benefits.

Optimize the amount of code an agent must discover, read, and keep in context to
complete a representative task. File length is a discovery signal, not the goal.
Splitting one operation across many tiny files can make this metric worse.
Keep invariants, validation, execution, and their focused tests easy to find
together. Keep explanatory comments about identity and process ownership.

## Scope and evidence limits

Inventoried tracked Rust, frontend, test, script, and site sources; inspected the
architecture/checks guidance, module declarations, central state and constructors,
GUI contracts and commands, diff rendering/input, headless execution, and selected
workflow implementations. Traced GUI draft submission, review polling, PR Triage
action validation/prefetch/application, and plan-interview state ownership.
This is a structural assessment, not an exhaustive correctness or security audit.
Scripts and the site received inventory-level review, not a full implementation
review. No runtime, screenshot, build, or test execution was needed for this
documentation-only investigation; existing test files were inspected, not run.

Measurements below count physical lines, including whitespace and comments. They
are not tokenizer measurements and do not establish a percentage token saving.
The production-prefix column stops before a trailing `#[cfg(test)] mod tests`;
it can still contain isolated test-only helpers.

| File | Total lines | Before trailing test module |
| --- | ---: | ---: |
| `src/app/state.rs` | 6,064 | 5,135 |
| `src/app/mod.rs` | 4,521 | Mixed implementation and named test modules |
| `gui/src/App.tsx` | 3,111 | 3,111 |
| `src/gui_contract.rs` | 5,142 | 2,360 |
| `src/gui_pr_triage.rs` | 5,581 | 3,092 |
| `src/ui/dialogs/diff.rs` | 6,634 | 5,272 |
| `src/handlers/diff.rs` | 4,870 | 864 |
| `src/headless.rs` | 4,152 | 2,265 |
| `src/app/ai_review.rs` | 4,287 | 3,349 |
| `src/app/plan_interview.rs` | 3,725 | 3,492 |
| `gui/src-tauri/src/main.rs` | 3,080 | 2,991 |

Other useful signals: `App` has 150 fields; Tauri main declares 176 commands;
`src/app/tests/pr_triage.rs` has 9,538 lines. These identify places to inspect,
not limits to enforce. Counts are reproducible with `git ls-files`, line counts,
and declaration searches at the revision above.

## Prioritized findings

### 1. Make architectural navigation current and task-oriented

**Priority: first. Confidence: high. Risk: low.**

[`architecture.md`](architecture.md) combines the general dependency map with
lengthy feature-specific workflow details. Its runtime-ownership section says
App has 125 fields, versus 150 now. The deferred store-overwrite discussion also
needs reconciliation with the version-checked save path now present in
[`db/store.rs`](../../src/db/store.rs) (`save_checked`) and `App::save`; this is
evidence of documentation drift, not proof that every persistence issue is fixed.
[`checks.md`](checks.md) still includes old milestone-specific preservation
instructions alongside ongoing contribution guidance. Its dated test results
are historical evidence, not a current validation baseline.

Keep a short current architecture entrypoint with dependency directions, shared
invariants, and a task-to-owner/test map. Move detailed workflow explanations to
linked feature documents; preserve historical inventories under `baseline/`.
Do not inject all feature documentation into root `AGENTS.md`. Link to it on demand.
This reduces repeated orientation cost without changing code.

### 2. Extract frontend workflow owners from App.tsx

**Priority: early. Confidence: high. Risk: medium.**

[`App.tsx`](../../gui/src/App.tsx) contains shell routing, workspace queries,
session drafts, lifecycle mutations, Learning/Review polling, many dialog states,
project/feature views, and creation forms. The extraction boundaries already
exist in behavior: `sendDraft` around line 687 owns session-targeted delivery and
draft preservation; Learning polling starts around 723; Review polling around
751 has different completion and revision semantics. Creation forms begin around
2807 and 2936 and are imported from App by multiple tests.

First move creation forms and project/feature views into named components with
explicit props. Then extract `useSessionDrafts`, `useLearningWorkflow`, and
`useReviewWorkflow`, each owning its state, effects, busy refs, and actions.
Keep workspace selection and cross-feature handoff in the shell. Do not replace
the file with a generic context carrying the same large state surface.

Preserve synchronous in-flight guards, originating-session attribution, appended
draft text, workflow/revision checks, and one-shot completion collection. Review
and Learning polling are similar but not interchangeable; premature generic
polling would conceal their different lifecycle rules.

### 3. Move feature state and behavior out of shared app/state.rs

**Priority: early. Confidence: high. Risk: medium.**

[`state.rs`](../../src/app/state.rs) contains unrelated Compose, prompt-library,
TODO, creation, session, and plan state. The plan section includes
`PlanInterviewState` around line 3334 and extensive behavior through line 4991:
answer editing, advancement, backtracking, critique, and completion. An agent
working on interviews must discover this behavior separately from
[`app/plan_interview.rs`](../../src/app/plan_interview.rs) and the root
[`plan_interview.rs`](../../src/plan_interview.rs).

Start by moving the cohesive plan types and methods from `app/state.rs` into
`app/plan_interview_state.rs`; keep the root `plan_interview.rs` domain/prompt
role explicit. Next move TODO and Compose state alongside their owners. Retain
central `AppMode` as the routing boundary and explicit re-exports for existing
imports.
Avoid a directory of arbitrary `state_part_N` fragments. Preserve pending launch,
paused interview, pre-call restoration, and persisted answer semantics.

### 4. Separate App construction from pane/sidebar runtime machinery

**Priority: after feature-state extraction. Confidence: high. Risk: high.**

[`app/mod.rs`](../../src/app/mod.rs) contains configuration, the 150-field App,
snapshot delivery/worker logic, sidebar queues/caches, construction, and shared
operations. Relevant anchors are `SnapshotSender` around 898,
`SidebarLoadExecutor` around 1361, snapshot-worker startup around 2345,
production/test construction around 2550/2829, and sidebar scheduling around 3109.
Changing one worker currently requires searching a large general-purpose module.

First mechanically move implementation into named pane-runtime, sidebar-runtime,
configuration, and construction modules. Then group fields only where the fields
share an actual lifecycle, following `PrReviewWork` and `LearningRuns`. Use common
defaults for the same production/test state; do not unify constructors by starting
real services in tests. Retain owned wakeup descriptors, worker stop signals,
queue/coalescing behavior, target matching, and shutdown order.

### 5. Split GUI adapters and command transport by feature

**Priority: early mechanical work, lifecycle work later. Confidence: high. Risk: medium.**

[`gui_contract.rs`](../../src/gui_contract.rs) is both a common DTO/facade boundary
and an implementation home for session recovery, feature lifecycle, and TODO
launching. TODO operations occupy approximately lines 1125–1748 despite the
existence of `gui_todos.rs`. `GuiHandle` should remain the boundary that hides App;
its implementation need not remain in one file.

Move common errors/targets, snapshot projection, session operations, and TODO
launch operations to named children or existing owners. Preserve facade exports
while callers migrate. Split [`Tauri main`](../../gui/src-tauri/src/main.rs) into
feature command modules, retaining app setup and an explicit command registry in
main. Preserve command names, serialization, locking, and blocking-thread behavior.
Avoid a new generic command framework just to remove repetitive wrappers.

[`gui_pr_triage.rs`](../../src/gui_pr_triage.rs) has natural internal seams:
contract/view projection, GitHub read planning, fix handoff, batch fix, and
investigation. Keep the orchestration gate and plan/apply entrypoints visible.
In particular, `plan_act` around 2162 and `act_prefetched` around 2219 validate
before and after unlocked reads. Moving these must not collapse the second check.
Keep batch digest, confirmation, draft rollback, and delivery handling together.

Rust and TypeScript contracts are manually mirrored (`gui/src/api.ts` and
`prTriageApi.ts` explicitly say so). Consider generated DTOs or serialization
fixtures after ownership improves; first demonstrate that this removes recurring
duplication without obscuring wire names, optional fields, or tagged unions.

### 6. Split diff rendering by coherent responsibilities

**Priority: early. Confidence: high. Risk: medium.**

[`ui/dialogs/diff.rs`](../../src/ui/dialogs/diff.rs) has roughly 5,272 lines before
its main test module. It combines scope selection, review overlays, file trees,
help/footers, cached patch rendering, split layout, highlighting, and wrapping.
Useful seams include `draw_patch_panel` around 2587, footer logic around 2885,
patch/split line construction around 3889/4120, and chunk styling/wrapping around
4677. Create a `diff/` renderer module with cohesive children such as `patch`,
`file_tree`, `footer`, and `overlays`, with tests beside the relevant behavior.

Retain a small public render facade and narrow shared layout types. Preserve
display-width calculations, cursor mapping, cache keys, and scroll extents.
The existing `highlight` and `worddiff` services should remain shared owners.
Do not move presentation-only helpers into App or introduce process/I/O work here.

By contrast, [`handlers/diff.rs`](../../src/handlers/diff.rs) has only 864 lines
before tests. Its first improvement is test organization, not a major dispatcher
redesign based on the misleading total file length.

### 7. Make tests as discoverable as production behavior

**Priority: alongside each owner extraction. Confidence: high. Risk: low to medium.**

`app/tests/pr_triage.rs` mixes named fix sessions, AI-review behavior, reply
handling, investigations, PR review posting, and acceptance workflows. It is
9,538 lines despite existing focused suites such as `pr_workspace` and
`review_questions`. The inline diff-handler tests alone occupy about 4,000 lines.

Split these suites by behavior (fix handoff, replies, investigations, PR review
submission) and move only genuinely shared fixtures to scoped support. Keep small
unit tests beside their implementations. Moving every test into a separate file
would increase navigation cost unnecessarily. Update imports/tests that currently
reach creation forms through `App.tsx` when those forms move.

Capture the current runnable test names before moves and compare transformed
sets afterward. The historical 2,511-test inventory is not a substitute for this
branch's baseline. Preserve assertions, per-test temp/DB/process lifetimes,
parallel execution, and private visibility.

### 8. Extract pure transformations before broader backend abstractions

**Priority: second wave. Confidence: high for seams, benefit unmeasured. Risk: medium.**

[`app/ai_review.rs`](../../src/app/ai_review.rs) combines output parsing/location
mapping, attribution, prompt construction, model selection, batch execution,
and App workflow operations. Parsing around 774–956 is a good initial boundary;
keep these functions data-oriented with their tests.

[`headless.rs`](../../src/headless.rs) combines harness command construction,
bounded process execution, JSONL event normalization, model/reasoning arguments,
capability probing, and prompt budgets. Extract protocol normalization and command
building before touching process ownership. Keep child guards, stream draining,
cancellation, deadlines, and resource leases together in the runner.

Prefer existing seams (`TmuxOps`, `WorktreeOps`, `GithubTransport`, `TriageGithub`)
over a new all-purpose service container. Introduce reuse where consumers share
the same semantics, not merely similar code shapes.

## Suggested implementation order and acceptance

1. Refresh the navigation guide and record representative task baselines.
2. Extract frontend creation forms and a single lifecycle owner; extract plan
   state separately. These are independent, reviewable changes.
3. Split diff rendering and oversized test suites by responsibility.
4. Split GUI facade/command modules, then PR Triage adapter internals.
5. Extract AI-review/headless pure logic; tackle App runtime ownership last.

Keep file movement and behavioral redesign in separate changes. Preserve public
wire contracts, persisted formats, keybindings, request identities, and process
lifetimes. Run focused existing suites during each extraction, then the full
parallel workspace tests, formatting, strict Clippy, and relevant GUI tests/build
at code milestones as required by [checks.md](checks.md).

For each proposed split, compare identical tasks before and after:

| Representative task | Current starting points | Relevant existing coverage |
| --- | --- | --- |
| Change plan answer/back behavior | `app/state.rs`, `app/plan_interview.rs`, root `plan_interview.rs` | `app::tests::plans`, plan handler/state tests, `gui/tests/PlanPanel.test.tsx` |
| Change draft delivery/preservation | `gui/src/App.tsx`, `api.ts`, Tauri terminal commands | `gui/tests/PromptComposer.test.tsx`, `App.test.tsx`, terminal tests |
| Change review footer or wrapping | `ui/dialogs/diff.rs`, review state; handlers for input behavior | Renderer tests and `handlers::diff` tests |
| Change PR batch-fix confirmation | `gui_pr_triage.rs`, PR domain/integration, destination/session guards | Adapter tests, `app::tests::pr_triage`, `gui/tests/PrTriagePanel.test.tsx` |

Record bytes/tokens actually returned by searches and file reads, number of
discovery steps, unrelated content loaded, and files changed. Include test and
documentation reads. Use the same tokenizer/model and task wording for token
comparisons. Accept an extraction when the task needs less unrelated context and
its invariants remain locally understandable. A smaller entrypoint that requires
reading every child module is not a demonstrated improvement.

Avoid global file-size caps, renaming every root `gui_*` module, a crate-per-feature
rewrite, blanket test separation, comment stripping, or generic workflow engines.
The existing feature ownership pattern is sufficient to make incremental progress.


## Implementation progress

First increment (2026-11-04):

- Replaced the architecture entrypoint with a task-to-owner/test map and shared
  invariants. Moved detailed GUI workflows and feature/runtime ownership to
  linked references. Preserved the GUI workflow paragraphs, adding headings.
  Removed the stale current field count; clarified historical validation and
  current test-inventory requirements in the development checks.
- Moved `CreateProjectForm` and `CreateFeatureForm` from `App.tsx` into their own
  modules, including exclusive helpers/imports. Tests now import the component
  owners directly, including the already-separated `TodoNewFeatureForm`.
  Form/helper bodies and test assertions are unchanged.
- `App.tsx`: 3,111 → 2,787 lines. Project creation now has a 139-line entrypoint;
  feature creation has a 187-line entrypoint, with explicit existing dependencies.
  Architecture entrypoint: 306 → 106 lines (20,117 → 7,353 UTF-8 bytes).
  These are source-locality measurements, not measured end-to-end token savings.
- Validation: all 750 GUI tests across 80 files passed; TypeScript/Vite production
  build passed. An independent mechanical review found no behavioral changes.
  Relative documentation links and whitespace were checked.

The frontend workflow hooks, plan-state extraction, diff rendering, GUI backend
modules, and App runtime ownership remain pending. No process ownership,
persistence format, Rust behavior, or UI workflow was changed in this increment.

Second increment (2026-11-05):

- Moved `ProjectView` into `gui/src/ProjectView.tsx`, and the shared page header,
  lifecycle type, and start/stop control into `gui/src/WorkspaceControls.tsx`.
  Moved bodies were preserved; `App.tsx` remains the composition and routing
  owner. It fell from 2,787 to 2,592 lines.
- Validation: production GUI build passed; focused `App`, sidebar workflow, and
  session controls suites passed (76 tests). `git diff --check` passed.
- These extractions reduce App-local context for page/control changes. Remaining
  frontend workflow hooks, plan state, diff rendering, GUI backend modules, and
  App runtime ownership still need separate investigation; no token-count claim
  has been measured.
- Assessed `FeatureView` as the next GUI split candidate. It is 435 lines, but
  takes 43 values/callbacks from `App` and has one call site. Moving it as-is
  would relocate markup while retaining a broad dependency surface, so defer
  that move. Prefer first identifying a cohesive workflow/state owner that can
  shrink the props the page needs; revisit the view after that boundary exists.

Third increment (2026-11-05):

- Moved per-session draft, send-state, and composer-focus state plus stale-key
  cleanup into `gui/src/sessionDraftState.ts`. The hook preserves draft keys for
  sessions awaiting snapshot confirmation after launch. `App` still owns prompt
  send actions and handoff behavior; the hook owns only the state lifecycle.
- Validation: GUI production build passed; `App.test.tsx` passed (71 tests).
  `git diff --check` passed. An initial run exposed that the hook was called
  before the workspace query declaration; moved the call after that declaration
  and reran both checks successfully.

Fourth increment (2026-11-05):

- Moved the newly launched session grace map, timeout, and snapshot-arrival
  cleanup into `gui/src/pendingSessionState.ts`. `App` continues to own tab
  selection and draft handoff. `sessionDraftState` consumes the pending map to
  keep handoff drafts alive until the session appears.
- Validation: GUI production build passed; `App.test.tsx` passed (71 tests).
  `git diff --check` passed.

Seventh increment (2026-11-05):

- Moved tracking and cleanup of running features' launched-mode overrides into
  `gui/src/launchedModeState.ts`. App mutations still update the overrides when
  a mode changes; the hook drops overrides when a feature stops.
- Validation: GUI production build passed; the focused App vibe-mode test passed
  (1 test, 70 skipped). `git diff --check` passed.

Eighth increment (2026-11-05):

- Moved Learning workflow state, revision-safe polling, action locking, actions,
  approval handling, and launch handoff into `gui/src/useLearningWorkflow.ts`.
  App supplies only cross-workflow effects: opening/starting the target session,
  error reporting, and user notices.
- Validation: GUI production build passed; `LearningWorkflow.test.tsx` passed
  (4 tests). `git diff --check` passed.

Ninth increment (2026-11-05):

- Moved Final Review state, workflow polling, revision guards, action locking,
  completion collection, syntax refresh, and review actions into
  `gui/src/useReviewWorkflow.ts`. App supplies error reporting, session handoff,
  and the completion notice.
- Validation: GUI production build passed; three focused review polling/action
  tests passed. `git diff --check` passed.

Tenth increment (2026-11-05):

- Moved draft editing guards, send locking, terminal submission, error handling,
  steering close, and originating-draft preservation into
  `gui/src/useSessionDraftActions.ts`. State lifecycle remains in
  `sessionDraftState`; handoff appending remains with the workspace shell.
- Validation: GUI production build passed; three focused App draft/send tests
  passed. `git diff --check` passed.

Eleventh increment (2026-11-05):

- Moved plan interview state types and their implementation from
  `src/app/state.rs` to `src/app/plan_interview_state.rs`. Re-exported the moved
  API through `app::state` to preserve existing imports. Existing plan tests
  remain next to their current shared test module; one implementation helper is
  visible to the parent app module for those tests.
- Validation: 146 plan-related Rust tests passed; `cargo check --locked` and
  `cargo fmt --check` passed. Removed unused imports reported by the first test
  compile; the 146-test run preceded that import-only cleanup.

Twelfth increment (2026-11-05):

- Moved TODO editor, view, launch, host-reassignment, deletion, and reference
  completion state types into `src/app/todo_state.rs`. `AppMode` remains in
  shared state as the routing boundary. Existing TODO tests remain in the
  shared state test module for now.
- Validation: `cargo check --tests --locked` and `cargo fmt` passed. No TODO
  test binary was run for this move; plan-related tests passed in the preceding
  increment.

Thirteenth increment (2026-11-05):

- Moved Compose command-source, command-entry, image, draft, and editor state
  types into `src/app/compose_state.rs`; shared-state consumers keep using the
  `app::state` re-export.
- Validation: `cargo check --tests --locked` passed. `cargo fmt` passed. This
  move did not run a Rust test binary.

Fourteenth increment (2026-11-05):

- Moved latest-prompt recall, prompt-library, prompt-editor, and placeholder
  state into `src/app/prompt_library_state.rs`; prompt-override state remains in
  shared app state for a separate ownership decision.
- Validation: `cargo check --tests --locked` and `cargo fmt` passed. This move
  did not run a Rust test binary.

Fifteenth increment (2026-11-05):

- Moved prompt-override scope, editor, picker, and manager state into
  `src/app/prompt_override_state.rs`; runtime actions remain in
  `app/prompt_overrides.rs`.
- Validation: `cargo check --tests --locked` and `cargo fmt --check` passed
  after removing one unused import. No Rust test binary was run.

Sixteenth increment (2026-11-05):

- Moved selected project/feature derivation, view state, and snapshot-driven
  selection recovery into `gui/src/useWorkspaceSelection.ts`. `App` keeps
  navigation actions and feature-specific side effects.
- Validation: GUI production build passed; the focused App navigation/draft
  test passed. `git diff --check` passed.

Seventeenth increment (2026-11-05):

- Moved Config Wizard categories, scope/step types, editor helpers, and state
  into the existing `app/config_wizard.rs` owner beside its behavior. Shared
  state re-exports preserve current callers.
- Validation: `cargo check --tests --locked` and `cargo fmt` passed. No Rust
  test binary was run.

Fifth increment (2026-11-05):

- Moved the workspace filter's `/` keyboard shortcut into
  `gui/src/workspaceFilterShortcut.ts`. `App` supplies the input ref; the hook
  owns event filtering, focus, selection, and listener cleanup.
- Validation: GUI production build passed; `SidebarWorkflow.test.tsx` passed
  (3 tests). `git diff --check` passed.

Sixth increment (2026-11-05):

- Moved the workspace snapshot query and `workspace-changed` event subscription
  into `gui/src/workspaceSnapshot.ts`. That hook now owns the query/poll/event
  synchronization contract; `App` retains the cache mutations and invalidations
  used by its workflows.
- Validation: GUI production build passed; `App.test.tsx` passed (71 tests).
  `git diff --check` passed.

Eighteenth increment (2026-11-05):

- Moved the sidebar load queue, bounded worker pool, shutdown behavior, and
  concurrency test into `src/app/sidebar_runtime.rs`. Sidebar result projection
  and task scheduling remain with `App` while their lifecycle is traced.
- Validation: the focused executor concurrency test passed; formatting passed.

Nineteenth increment (2026-11-05):

- Moved the latest-wins pane snapshot sender and same-target field merge into
  `src/app/pane_runtime.rs`. `App` retains pipe creation, receiver ownership,
  and worker lifecycle.
- Validation: `cargo check --tests --locked` and `cargo fmt --check` passed.

Twentieth increment (2026-11-05):

- Centralized snapshot mailbox and nonblocking wakeup-pipe creation in
  `pane_runtime::SnapshotSender`; production and test constructors now share
  the same owner. `App` retains the read descriptor and pane worker lifecycle.
- Validation: `cargo check --tests --locked` and formatting passed.

Twenty-first increment (2026-11-05):

- Moved pane target selection, worker start/stop orchestration, and refresh
  requests into `pane_runtime.rs`. Snapshot drain and display updates stay in
  `App` because they apply the frame to the visible UI.
- Validation: `cargo check --tests --locked` and `cargo fmt` passed.

Twenty-second increment (2026-11-05):

- Moved capture-pane, control-mode, and pipe-pane worker bodies into
  `pane_runtime.rs` with their refresh orchestration. Pane parsing, wakeup,
  worker lifecycle, and refresh policy now have one named owner.
- Validation: `cargo check --tests --locked` and `cargo fmt` passed.

Twenty-third increment (2026-11-05):

- Moved production and test App initialization into `app/construction.rs`.
  The constructors remain distinct: production starts required services, while
  test construction uses injected tmux/worktree operations and shared cache
  helpers.
- Validation: `cargo check --tests --locked` and `cargo fmt` passed.

Twenty-fourth increment (2026-11-05):

- Grouped pane snapshot transport, worker signals, target, receiver, and display
  freeze deadline in `PaneRuntime`. These fields share worker lifecycle and
  remain initialized consistently by production and test construction.
- Validation: `cargo check --tests --locked` passed; passing `cargo fmt --check`.

Twenty-fifth increment (2026-11-05):

- Grouped sidebar load channels, worker pool, per-session signatures, and
  pending-session tracking in `SidebarRuntime`. Production and test constructors
  now share its initialization; tests follow the grouped state.
- Validation: `cargo check --tests --locked`, `cargo fmt --check`, and
  `git diff --check` passed.

Twenty-sixth increment (2026-11-05):

- Moved GUI TODO target resolution, quick capture, agent launch, new-feature
  handoff, and rollback methods into `gui_contract/todo_launch.rs`. Persistence
  adapters remain in `gui_todos.rs`; `GuiHandle` remains the public facade.
- Validation: `cargo check --tests --locked` and `cargo fmt` passed.

Twenty-seventh increment (2026-11-05):

- Moved saved-session discovery, recovery choices, recovery rollback, and
  single-session stop/window handling into `gui_contract/session_lifecycle.rs`.
  These remain `GuiHandle` methods and keep their existing request/response API.
- Validation: `cargo check --tests --locked` and `cargo fmt` passed.

Twenty-eighth increment (2026-11-05):

- Moved feature TODO scope resolution plus start, approval, and stop adapters into
  the existing `gui_feature_actions.rs` owner. Shared DTOs and behavior remain
  unchanged at the `GuiHandle` boundary.
- Validation: `cargo check --tests --locked` and `cargo fmt` passed.

Twenty-ninth increment (2026-11-05):

- Moved bookmark and needs-attention Tauri commands into
  `gui/src-tauri/src/commands/bookmarks.rs`; the app setup retains explicit
  command registration. Command names and handler bindings are unchanged.
- Validation: `cargo check --manifest-path gui/src-tauri/Cargo.toml --locked`
  and Tauri formatting passed.

Thirtieth increment (2026-11-05):

- Moved prompt library, user prompt, declarative prompt-file, and export Tauri
  commands into `commands/prompts.rs`. The main handler registry remains the
  explicit list of command names.
- Validation: locked Tauri `cargo check` and formatting passed.

Thirty-first increment (2026-11-05):

- Added prompt override load/save/clear/target commands to `commands/prompts.rs`.
  All prompt library and override IPC wiring now shares one feature owner.
- Validation: locked Tauri `cargo check` and formatting passed.

Thirty-second increment (2026-11-05):

- Moved Tauri session recovery, start/stop, removal, and feature deletion
  handlers into `commands/sessions.rs`. The main handler registry remains
  explicit and keeps each IPC command name.
- Validation: locked Tauri `cargo check` and formatting passed.

Thirty-third increment (2026-11-05):

- Moved TODO list, edit, scratchpad, next-item, quick-capture, work-target, and
  agent-launch Tauri commands into `commands/todos.rs`.
- Validation: locked Tauri `cargo check` and formatting passed.

Thirty-fourth increment (2026-11-05):

- Moved plan-interview and Learning workflow Tauri commands into
  `commands/plans.rs` and `commands/learning.rs`. Both modules own their IPC
  parameter and response wiring; command registration remains in main.
- Validation: locked Tauri `cargo check` and formatting passed.

Thirty-fifth increment (2026-11-05):

- Moved review workflow, destination, completion, and action Tauri commands into
  `commands/review.rs`; the main registry still lists every command explicitly.
- Validation: locked Tauri `cargo check` and formatting passed.

Thirty-sixth increment (2026-11-05):

- Moved supervised-edit explanation, count, load, and response Tauri commands
  into `commands/supervised_edits.rs`. The explanation controller remains
  managed by the app setup and is passed to that command owner.
- Validation: locked Tauri `cargo check` and formatting passed.

Thirty-seventh increment (2026-11-05):

- Moved GitHub issue browser and fix-preview/create Tauri commands, including
  their blocking-thread helper, into `commands/issues.rs`.
- Validation: locked Tauri `cargo check` and formatting passed.

Thirty-eighth increment (2026-11-05):

- Moved syntax installation and language-management Tauri commands into
  `commands/syntax.rs`.
- Validation: locked Tauri `cargo check` and formatting passed.

Thirty-ninth increment (2026-11-05):

- Moved terminal attachment state, generation-safe replacement, and terminal
  Tauri commands into `commands/terminal.rs`. App setup now constructs the
  terminal owner through one initializer.
- Validation: locked Tauri `cargo check` and formatting passed.

Fortieth increment (2026-11-05):

- Moved project/feature creation and worktree-hook-prompt Tauri commands into
  `commands/features.rs`; feature start registration sits with session actions.
- Validation: locked Tauri `cargo check` and formatting passed.

Forty-first increment (2026-11-05):

- Moved preset, context, agent availability, and project harness preference
  commands into `commands/settings.rs`, including the blocking probe helper.
- Validation: locked Tauri `cargo check` and formatting passed.

Forty-second increment (2026-11-05):

- Added new-session options, session creation, custom sessions, VS Code/editor
  operations, and feature start to `commands/sessions.rs`.
- Validation: locked Tauri `cargo check` and formatting passed.

Forty-third increment (2026-11-05):

- Added feature harness load/apply handlers to the existing `commands/features.rs`
  owner.
- Validation: locked Tauri `cargo check` and formatting passed.

Forty-fourth increment (2026-11-05):

- Moved fork prepare/confirm/cancel/status commands and their blocking helper
  into `commands/fork.rs`.
- Validation: locked Tauri `cargo check` and formatting passed.

Forty-fifth increment (2026-11-05):

- Moved PR review and PR triage command handlers, including GitHub read
  prefetching, into `commands/pull_requests.rs`. Companion integration state
  remains in the main module with its consumers and tests.
- Validation: locked Tauri `cargo check` and formatting passed.

Forty-sixth increment (2026-11-05):

- Moved PR workspace workflow, fix-target, file-view, and dedicated-session
  catalog Tauri commands into `commands/pr_workspace.rs`. The blocking helper
  stays shared in main because companion integration also uses it.
- Validation: locked Tauri `cargo check` and formatting passed.

Forty-seventh increment (2026-11-05):

- Moved local, remote, cleanup, browser, and inline screenshot Tauri commands
  into `commands/screenshots.rs`.
- Validation: locked Tauri `cargo check` and formatting passed.

Forty-eighth increment (2026-11-05):

- Added diff-layout and dormancy settings/lifecycle commands to
  `commands/settings.rs`, alongside global and project settings.
- Validation: locked Tauri `cargo check` and formatting passed.

Forty-ninth increment (2026-11-05):

- Moved session sidebar and fresh-context Tauri commands into
  `commands/session_sidebar.rs`; that module now owns its sidebar source clock.
- Validation: locked Tauri `cargo check` and formatting passed.

Fiftieth increment (2026-11-05):

- Moved companion integration locking/status/preview/confirm commands and their
  exclusivity test into `commands/companion_integration.rs`. PR-destination,
  companion-session, and preparation commands now live in
  `commands/pr_destinations.rs`.
- Validation: locked Tauri `cargo check` and formatting passed.

Fifty-first increment (2026-11-05):

- Kept terminal attachment generation coverage beside `commands/terminal.rs`
  after moving the terminal registry owner out of main.
- Validation: locked Tauri `cargo check --tests`, formatting, and
  `git diff --check` passed.

Fifty-second increment (2026-11-05):

- Moved diff scope picker rendering into `diff_picker.rs`; the existing public
  `draw_diff_picker` entry point remains re-exported from `diff.rs`.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed.

Fifty-third increment (2026-11-05):

- Moved diff footer rendering, hint fitting, comment preview, and footer sizing
  into `diff_footer.rs`; the main renderer keeps the footer entry point and
  shared height calculation.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Fifty-fourth increment (2026-11-05):

- Moved diff header rendering and its PR-scope/review-count helpers into
  `diff_header.rs`; the renderer retains the same header entry point.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Fifty-fifth increment (2026-11-05):

- Moved diff file-tree rows, directory summaries, test/source path recognition,
  and risk markers into `diff_file_tree.rs`; the dialogs-level draw entry point
  remains available to the PR workspace renderer.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Fifty-sixth increment (2026-11-05):

- Moved review cursor markers and comment lookup/preview helpers into
  `diff_comments.rs`, leaving patch-panel and cached-row ownership in the main
  renderer pending a dedicated patch extraction.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Fifty-seventh increment (2026-11-05):

- Moved diff submit/help/history/interdiff/summary/search/base-ref overlays and
  their presentation helpers into `diff_overlays.rs`; the viewer keeps explicit
  overlay calls and the help data remains available to focused tests.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Fifty-eighth increment (2026-11-05):

- Moved diff chunk styling, highlight composition, and display-width wrapping into
  `diff_text.rs`; the renderer imports only the helpers it calls directly.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Fifty-ninth increment (2026-11-05):

- Replaced the diff picker renderer's wildcard parent import with its explicit
  Ratatui, state, theme, and modal dependencies.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed.

Sixtieth increment (2026-11-05):

- Replaced the diff header renderer's wildcard parent import with its explicit
  Ratatui, viewer-state, and theme dependencies.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed.

Sixty-first increment (2026-11-05):

- Replaced the review-comment renderer helpers' wildcard parent import with
  explicit viewer-state and diff-location dependencies.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed.

Sixty-second increment (2026-11-05):

- Replaced diff text styling and wrapping helpers' wildcard parent import with
  explicit diff, highlighting, theme, and display-width dependencies.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed.

Sixty-third increment (2026-11-05):

- Replaced the diff file-tree module's wildcard parent import with explicit
  state, diff, theme, Ratatui, and shared status-label dependencies.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed.

Sixty-fourth increment (2026-11-05):

- Replaced the diff footer module's wildcard parent import with explicit viewer,
  editor, highlighting, width, Ratatui, and shared comment/layout dependencies.
- Removed the corresponding unused root imports.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed.

Sixty-fifth increment (2026-11-05):

- Replaced the diff body renderer's wildcard parent import with explicit state,
  layout, theme, and file/patch rendering dependencies.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed.

Sixty-sixth increment (2026-11-05):

- Replaced the overlay module's wildcard parent import with explicit state,
  theme, dialog, and Ratatui dependencies, including the shared patch panel.
- Removed root imports used only by overlays.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed.

Sixty-seventh increment (2026-11-05):

- Moved PR Triage's serialized view DTOs into `gui_pr_triage/types.rs` and
  re-exported them from the adapter. Kept pending GitHub write state with the
  workflow owner.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Sixty-eighth increment (2026-11-05):

- Moved the pure PR review-to-view projection and sort labels into
  `gui_pr_triage/projection.rs`; the adapter calls the projection from its view
  builder.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Sixty-ninth increment (2026-11-05):

- Moved AI Review attribution formatting, token usage construction, and body
  marker normalization into `app/ai_review/attribution.rs`; existing callers
  retain the same helper names.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Seventieth increment (2026-11-05):

- Moved AI Review usage formatting and attribution body normalization into
  `app/ai_review/attribution.rs`.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed.

Seventy-first increment (2026-11-05):

- Moved AI Review diff annotation and response parsing into
  `app/ai_review/parser.rs`; the prompt and processing paths retain their
  existing helper names.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Seventy-second increment (2026-11-05):

- Moved AI Review source-coordinate resolution and hunk reconstruction into
  `app/ai_review/locations.rs`; the app-level hunk helper remains re-exported
  for review questions.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Seventy-third increment (2026-11-05):

- Moved prompt-size estimation, built-in harness budgets, overflow checks, and
  typed prompt-too-long errors into `headless/prompt_limits.rs`, re-exported
  from `headless` for existing callers.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Seventy-fourth increment (2026-11-05):

- Moved headless reasoning-level and model-argument assembly into
  `headless/invocation.rs`, re-exporting public settings types while leaving
  command execution and child lifetimes in `headless.rs`.
- Validation: locked library `cargo check`, formatting, and `git diff --check`
  passed. Tests were not run.

Seventy-fifth increment (2026-11-05):

- Corrected test-only imports for the extracted diff and AI Review helpers, and
  moved the feature-action adapter implementation ahead of its test module.
- Validation: workspace tests passed (4,178 library and 9 Tauri tests); strict
  Clippy and formatting passed. GUI build passed. The full GUI suite has one
  repeatable `PrWorkspacePanel` failure under the full run; that file passes
  when run alone (9/9).

Seventy-sixth increment (2026-11-06):

- Moved the inline diff-handler test module to `handlers/diff/tests.rs`. The
  production dispatcher is now 866 lines; its 112 tests remain in one sibling
  module with unchanged names and bodies.
- Validation: test-name comparison found the same 112 tests; `cargo check
  --tests --locked`, `cargo fmt`, and `git diff --check` passed. Tests were not
  executed.

Seventy-seventh increment (2026-11-06):

- Grouped 10 diff-handler viewport scrolling and help-overlay tests in
  `handlers/diff/tests/viewport_navigation.rs`; shared fixtures remain in the
  parent test module.
- Validation: all 112 test names are still present; `cargo check --tests
  --locked`, formatting, and `git diff --check` passed. Tests were not run.

Seventy-eighth increment (2026-11-06):

- Grouped diff search and context-expansion helpers with their six tests in
  `handlers/diff/tests/search.rs`; the shared search patch fixture stays with
  the adjacent editor tests.
- Validation: all 112 test names remain present and unique; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Seventy-ninth increment (2026-11-06):

- Grouped diff viewer `$EDITOR` handoff setup and four related tests in
  `handlers/diff/tests/editor_launch.rs`.
- Validation: all 112 test names remain present and unique; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Eightieth increment (2026-11-06):

- Grouped final-review finish confirmation, summary navigation, and pause
  behavior tests in `handlers/diff/tests/finish_flow.rs`.
- Validation: all 112 test names remain present and unique; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Eighty-first increment (2026-11-06):

- Grouped diff review comment and suggestion workflows, including Git-backed
  suggestion application, in `handlers/diff/tests/comment_suggestions.rs`.
  Shared fixtures used by other groups remain in the parent module.
- Validation: all 112 test names remain present and unique; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Eighty-second increment (2026-11-06):

- Grouped walkthrough request handling and changeset/interdiff overview modal
  tests in `handlers/diff/tests/overview.rs`.
- Validation: all 112 test names remain present and unique; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Eighty-third increment (2026-11-06):

- Grouped final-review layout, file navigation, and filtering tests in
  `handlers/diff/tests/file_navigation.rs`.
- Validation: all 112 test names remain present and unique; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Eighty-fourth increment (2026-11-06):

- Began splitting the oversized PR Triage integration suite. Moved repository
  resolution, list navigation, and return-to-pane tests to
  `app/tests/pr_triage/navigation.rs`; shared selection helper stays in the
  parent suite.
- Validation: the 220 PR Triage test names are unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Eighty-fifth increment (2026-11-06):

- Moved PR Triage investigation reconciliation, action, reply, and follow-up
  tests to `app/tests/pr_triage/investigations.rs`.
- Validation: all 220 PR Triage test names remain unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Eighty-sixth increment (2026-11-06):

- Grouped PR review fix handoff and per-comment targeting tests in
  `app/tests/pr_triage/fix_handoff.rs`; the shared AI-review finding fixture
  remains available from the parent suite.
- Validation: all 220 PR Triage test names remain unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Eighty-seventh increment (2026-11-06):

- Moved PR Review Memory bootstrap, compaction confirmation, background
  completion, and persistence tests to `app/tests/pr_triage/review_memory.rs`.
- Validation: all 220 PR Triage test names remain unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Eighty-eighth increment (2026-11-06):

- Grouped PR review dedicated-session status, usage accounting, and draft
  provenance tests in `app/tests/pr_triage/session_activity.rs`.
- Validation: all 220 PR Triage test names remain unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Eighty-ninth increment (2026-11-06):

- Grouped PR fix target selection, batch confirmation, and sibling navigation
  tests in `app/tests/pr_triage/fix_batch.rs`.
- Validation: all 220 PR Triage test names remain unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Ninetieth increment (2026-11-06):

- Grouped PR fix-confirm editor, scroll, target-switch, and target-picker tests
  in `app/tests/pr_triage/fix_editor.rs`.
- Validation: all 220 PR Triage test names remain unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Ninety-first increment (2026-11-06):

- Grouped PR comment ordering, conversation grouping, and resolved-item
  selection tests in `app/tests/pr_triage/review_sort.rs` with their scoped
  fixtures.
- Validation: all 220 PR Triage test names remain unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Ninety-second increment (2026-11-06):

- Grouped PR reply drafting, attribution, commit context, editing, and posting
  precondition tests in `app/tests/pr_triage/replies.rs`; the editor-text helper
  remains shared from the parent suite.
- Validation: all 220 PR Triage test names remain unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Ninety-third increment (2026-11-06):

- Grouped PR Review Memory add/edit, AI summary, and generated-content
  persistence tests in `app/tests/pr_triage/memory_workflows.rs`.
- Validation: all 220 PR Triage test names remain unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Ninety-fourth increment (2026-11-06):

- Grouped AI Review cost attribution and harness/model picker tests in
  `app/tests/pr_triage/ai_review_picker.rs`.
- Validation: all 220 PR Triage test names remain unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Ninety-fifth increment (2026-11-06):

- Grouped AI Review background execution, progress recovery, triage refresh,
  cached results, and post-dialog tests in
  `app/tests/pr_triage/ai_review_workflow.rs`.
- Validation: all 220 PR Triage test names remain unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.

Ninety-sixth increment (2026-11-06):

- Grouped PR fix-target companion feature creation, adoption, and integration
  tests in `app/tests/pr_triage/companion_feature.rs`.
- Validation: all 220 PR Triage test names remain unchanged; `cargo check
  --tests --locked`, formatting, and `git diff --check` passed. Tests were not
  run.
