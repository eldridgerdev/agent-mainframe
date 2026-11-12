# Architecture

AMF has `amf` terminal and `amf-gui` Tauri executables sharing the
`agent-mainframe` library, SQLite persistence, and tmux integration. Start with
this map, then read only the relevant owner and workflow reference.
See [checks.md](checks.md) for validation and [the organization audit](code-organization-audit.md)
for the staged refactoring plan. Historical inventories live under [baseline/](baseline/README.md).

## Runtime and dependency directions

```text
cli -> handlers -> App feature operations -> domain, persistence, external adapters
cli -> ui -> display state and presentation helpers
GUI components -> API contracts -> Tauri commands -> GUI adapters -> App operations
workers -> channels/mailbox -> poll/apply -> matching feature state
```

- `src/main.rs` delegates to `cli::run`. `src/cli.rs` owns startup, terminal
  teardown, event deadlines, worker polling, session reconciliation, and redraws.
- `src/handlers/mod.rs` routes by `AppMode`; `src/ui/dashboard.rs` dispatches
  renderers. Input handlers translate input into operations. Renderers own
  presentation; neither layer should acquire persistence or process launching.
- `src/app/mod.rs` constructs App/AppConfig and owns shared pane/sidebar runtime,
  configuration, and logging. Feature modules implement App orchestration.
  `src/app/state.rs` retains shared routing and some feature state; PR Triage,
  Final Review, and Learning already own their state in feature directories.
- `gui/src/App.tsx` owns shell navigation, cross-feature handoffs, and remaining
  frontend workflow orchestration. Components and feature API modules live in
  `gui/src/`; `gui/src-tauri/src/main.rs` binds commands to Rust adapters.
- `src/gui_contract.rs` owns common GUI targets/errors, snapshots, and `GuiHandle`,
  which hides a private App. `src/gui_*.rs` adapt shared operations by stable IDs.
  Preserve this shared engine instead of duplicating TUI behavior in the GUI.

## Find the owner for a task

Paths below are relative to `src/` unless prefixed with `gui/`. Within a cell,
abbreviated filenames or suite names inherit the preceding directory or module.
Test entries are existing suites or files; use the focused commands in
[checks.md](checks.md).

| Task | Start here | Relevant tests |
| --- | --- | --- |
| Startup, event timing, worker polling | `cli.rs`, `app/mod.rs` | `app::tests::startup_navigation`, `pane_input` |
| Feature/session creation, start, stop, recovery | `app/feature_ops.rs`, `app/session_ops.rs`, `gui_contract.rs`, `gui_sessions.rs` | `app::tests::feature_sessions`, `gui_sessions` |
| GUI creation forms | `gui/src/CreateProjectForm.tsx`, `CreateFeatureForm.tsx`, `useCreationHarness.ts` | `gui/tests/FirstRun.test.tsx`, `WorktreeHooks.test.tsx`, `OfferedAgents.test.tsx` |
| GUI project page and shared header/start-stop controls | `gui/src/ProjectView.tsx`, `WorkspaceControls.tsx` | `gui/tests/App.test.tsx`, `SidebarWorkflow.test.tsx`, `SessionControls.test.tsx` |
| GUI workspace selection and recovery | `gui/src/useWorkspaceSelection.ts`, `workspaceSnapshot.ts` | `gui/tests/App.test.tsx` |
| Session drafts and cross-feature handoff | `gui/src/useSessionDraftActions.ts`, `sessionDraftState.ts`, `App.tsx`, `PromptComposer.tsx` | `gui/tests/PromptComposer.test.tsx`, `App.test.tsx` |
| Plan questions, answer navigation, launch | `app/plan_interview.rs`, `app/plan_interview_state.rs`, `plan_interview.rs`, `gui_plans.rs` | `app::tests::plans`, `gui/tests/PlanPanel.test.tsx` |
| PR comments, replies, investigation, fix handoff | `app/pr_review/`, `gui_pr_triage.rs` | `app::pr_review`, `app::tests::pr_triage`, `gui_pr_triage` |
| Final Review and PR review drafts/submission | `app/review/`, `gui_review/`, `gui_pr_review.rs` | `app::review`, `app::tests::final_review`, `gui_pr_review` |
| Learning questions and session lifecycle | `app/learning/`, `gui_learning.rs` | `app::learning`, `gui/tests/LearningWorkflow.test.tsx` |
| Diff layout, wrapping, navigation | `ui/dialogs/diff.rs`, `handlers/diff.rs`, `app/review/state.rs` | `ui::dialogs::diff`, `handlers::diff` |
| Shared syntax and word differences | `highlight/`, `worddiff.rs`, `gui_syntax.rs` | `highlight`, `worddiff`, `gui/tests/SyntaxHighlighting.test.tsx` |
| TODO editing and launching | `app/todo_state.rs`, `app/todos.rs`, `db/todos.rs`, `gui_todos.rs`, `gui_contract.rs` | `app::tests::todos`, `gui/tests/TodoNext.test.tsx` |
| Compose editor, command state and [initial prompts](features/session-prompts.md) | `app/compose_state.rs`, `app/compose.rs`, `app/automation.rs` | `app::tests::pane_input`, `app::tests::automation`, `ui::dialogs::compose` |
| Prompt library and overrides | `app/prompt_library_state.rs`, `app/prompt_override_state.rs`, `prompt_library/`, `prompts/`, `gui_prompts/`, `gui_prompt_overrides.rs` | `app::tests::prompts_configuration`, `gui/tests/PromptLibraryPanel.test.tsx` |
| Config Wizard | `app/config_wizard.rs` | No focused App suite is currently defined |
| Harness command/protocol or headless process lifetime | `headless.rs`, `claude.rs`, `codex.rs`, `pi.rs`, `app/opencode.rs` | `headless`, corresponding harness module tests |
| Run a cloned project in a container | `container/` (`settings`, `args`, `egress`, `image`, `runtime`, `tmux`), `app/sandbox.rs`, `headless.rs` (untrusted-root guard) | `container`, `app::sandbox`, `headless::untrusted_root_tests` |
| Clone a project from a git URL | `git_clone/` (`url`, `dest`, `env`, `preflight`, `classify`, `runner`, `finalize`), `app/clone_project.rs`, `gui_clone_project.rs`; GUI via `gui_contract.rs` and `gui/src/CreateProjectForm.tsx` | `git_clone`, `app::clone_project`, `app::tests::clone_project_form`, `gui/tests/CloneProject.test.tsx` |
| Persistence and concurrent writers | `db/`, `app/feature_ops.rs`, `app/mod.rs` | `db`, `app::tests::store_sync` |
| Resource admission and owned processes | `resources/`, `app/resource_gate.rs`, `app/editor_ops.rs` | `app::tests::resources`, `editor_lifecycle` |

## Feature modules

Per-feature implementation notes and invariants live in [features/](features/):
TODOs, Learning Mode, the plan interview, headless prompts, batched review and
resource health. Read the matching note before changing that area.

[Feature ownership and runtime lifetimes](feature-ownership.md) maps the PR Triage,
Final Review, and Learning submodules and explains their different close/reopen
and stale-result rules. [GUI workflow boundaries](gui-workflows.md) describes
adapter confirmation, revision, delivery, and locking invariants by feature.

Keep pure transformations independent of App. Import helpers from their owning
module, expose only consumed entrypoints, and keep private feature helpers private.
Shared diff/layout concepts belong to ordinary diff browsing as well as reviews.
Do not absorb neighboring features merely to reduce file length.

`git_clone/` is the UI-free core for creating a project from an HTTPS or SSH git
URL: URL validation, destination derivation, a non-interactive git environment,
pre-flight checks, failure classification, a cancelable background clone runner
that owns and cleans up its destination, and registration. The TUI and GUI depend
on it through `app/clone_project.rs`, never the reverse.

## Runtime ownership and stale results

Keep one authoritative owner for state. AppMode remains the central routing
boundary; cross-feature transitions remain on App. Worker completion must check
its request, workflow, repository/head, or session identity before applying data.
Closing a view does not universally cancel work: consult the feature's lifecycle
before moving channels or child guards. Preserve process lifetimes, wakeups,
queue behavior, and cleanup order during extraction.

## Data and side effects

`project.rs` models projects, features, and sessions. `db/mod.rs` owns connections;
`db/migrations.rs` owns schema changes; feature DB modules own persistence.
`App::save` reports conflicting concurrent writes, backed by the version check
and write transaction in `db::store::save_checked`. Preserve that check and
stable-ID selection restoration when refreshing another process's changes.

`traits.rs` provides `TmuxOps` and `WorktreeOps`, backed by `tmux.rs` and
`worktree.rs`. `github.rs` owns `GithubTransport`/`GhCli`; PR Triage adds
`pr_review/github_access.rs`'s `TriageGithub` boundary. Headless execution,
IPC, filesystem observation, editors, and resource inspection have dedicated
modules. Injection remains partial: add adapters for concrete boundaries rather
than introducing an all-purpose service container.

## Tests

`app/tests/mod.rs` declares behavior suites; shared fixtures live in `support.rs`.
Feature-local unit suites stay with their owners. Share helpers only when several
suites consume them. Each test retains its temporary directories, databases,
process guards, and original lifetimes. Use `App::new_for_test` and shared group
defaults, and keep normal parallel execution. When moving tests, capture the
current runnable inventory and compare names through an explicit move map.

Historical reliability findings and past validation results are evidence about
their recorded revisions, not a current backlog or a green check for this branch.
Recheck the implementation before carrying them into a new change.
