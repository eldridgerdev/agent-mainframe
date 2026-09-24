# AMF GUI — workflow inventory and first-slice gate

Scopes Task 1 ("Define coverage and release gates") of `AMF_PLAN.md`. Maps
every TUI workflow to its shared Rust operations, its TUI entry point, and
its proposed GUI destination from the plan's UI section, and states the
first-slice gate and the TUI-regression policy that applies at every later
milestone.

## Current source capabilities (development preview)

These labels describe the code in this branch, not a packaged GUI release.
Keep them current as the staged GUI scope grows.

| Status | GUI workflow |
| --- | --- |
| Available | Project and feature creation, feature start/stop, session navigation and live tmux terminal attachment/reconnection. |
| Available | Global, project and worktree TODO lists: add, change status, delete, reorder, move and copy. Start a TODO agent in an existing feature or a new git worktree feature with an editable, unsent prompt. |
| Available | Full and Quick Plan on an existing feature or while creating a feature; Full Plan for a TODO in an existing feature or a new git worktree. Review/edit, headless-call notice, cancellation and explicit approval for over-limit starts. |
| Limited | Creation-time planning with any `on_worktree_created` hook, and ordinary or direct TODO creation with a choice-prompting worktree hook, still need the TUI wizard. Script-only hooks run in ordinary and direct TODO creation. |
| Planned | TODO disposition when a feature is deleted. |
| Planned | Learning, supervised edits, PR triage, final review, diff review, settings and the other workflows listed below. |

The Linux development build and automated suites pass, and the GUI has been
run under WSL2/WSLg during development, which is where attaching a terminal
from a desktop launch (no terminal on stdin) was fixed. Native macOS runtime
checks remain release work. Releases publish x86_64 and aarch64 Linux `.deb`
and AppImage builds; macOS packaging and signing are still open. The TUI remains
the complete interface for workflows marked Planned.

## Method

Enumerated `AppMode` (`src/app/state.rs`), its dispatch arm in
`src/handlers/mod.rs`, the matching `src/app/*.rs` module(s), and (where one
exists) the `src/ui/dialogs/*.rs` renderer. Checked presentation coupling by
grepping `src/app/**/*.rs` (excluding `tests/`) for `ratatui::`, `vt100::`,
`crossterm::event`, `KeyEvent`, `KeyCode`. Dashboard-level entry keys are
read from `src/handlers/normal.rs`; nested-mode entry points are read from
their own handler file. Feature groupings follow `docs/development/
architecture.md` and the feature sections of `CLAUDE.md`.

Presentation coupling column: **Clean** = no direct ratatui/vt100/crossterm
import found; **Coupled** = the module directly imports one or more of
those. Clean does not mean already GUI-ready (it may still return
`String`/enum values shaped for terminal display, hold `AppMode` unions, or
assume synchronous completion) — it means no *type-level* rewrite is forced
before a GUI adapter can call it.

## Workflow → shared ops → TUI entry → GUI destination

| Feature area | Representative `AppMode`(s) | TUI entry point | Shared ops module(s) | Coupling | Proposed GUI destination |
|---|---|---|---|---|---|
| Dashboard / project & feature lifecycle | `Normal`, `CreatingProject`, `CreatingFeature`, `DeletingProject`, `DeletingFeature(InProgress)`, `RenamingFeature` | Dashboard keys (`n`ew project flow, feature-creation wizard, `d`/`x` delete/rename via `normal.rs`) | `app/project_ops.rs`, `app/feature_ops.rs`, `app/rename.rs` | `project_ops.rs` **Coupled**; `feature_ops.rs` clean by this grep | Workspace navigation |
| Session start/stop/attach, session config | `Viewing`, `SessionConfig`, `ProjectAgentConfig`, `RenamingSession`, `SessionSwitcher`, `NamingNewSession` | `Enter`/`s` picker, session switcher, `normal.rs`, `handlers/view.rs` | `app/session_ops.rs`, `app/session_config.rs`, `app/switcher.rs`, `app/session_titles.rs` | Clean by this grep | Session workspace |
| Saved-transcript / harness session pickers | `ClaudeSessionPicker`/`ConfirmingClaudeSession`, `CodexSessionPicker`/`Confirming...`, `OpencodeSessionPicker`/`Confirming...`, `StoppedSessionDialog` | `s` picker fallback when a harness has resumable sessions | `app/claude_session_picker.rs`, `app/claude_sessions.rs`, `app/codex_session_picker.rs`, `app/codex_sessions.rs`, `app/opencode*.rs` | Clean by this grep | Session workspace |
| Compose / steering (live prompt injection) | `Compose`, `SteeringPrompt` | Leader-key compose in an embedded session view (`handlers/compose.rs`) | `app/compose.rs`, `app/steering.rs` | Clean by this grep | Session workspace |
| TODOs (worktree/project/global) | `Todos`, `TodoQuickCapture`, `TodoImplementChoice`, `TodoSpawnTarget`, `TodoDeleteDisposition`, `TodosHostReassign`, `ConfirmTodoReferenceCompletion` | `s` picker → Todos session; leader `N` quick-capture; `I` implement-next | `app/todos.rs`, `db/todos.rs` | Clean by this grep | TODOs |
| Learning Mode | `Learning` | Dashboard `K` | `app/learning/{lifecycle,navigation,workers,follow_up,state,runtime}.rs`, `db/learning.rs` | `learning/state.rs` **Coupled**; siblings clean | Learning |
| Plan interview (Full + Quick) | `PlanInterview`, `PlanInterviewAttachDoc` | Feature-creation wizard Plan field; dashboard `Q`/`P` | `app/plan_interview.rs`, `app/plan_interview_attach.rs`, `plan_interview.rs` (prompt builders), `db` plan-interview tables | `plan_interview_attach.rs` **Coupled**; `app/plan_interview.rs` clean by this grep | Planning and review |
| Final Review (diff walkthrough/co-review/comments) | `DiffViewer(Loading)`, `DiffPicker`, `ReviewHarnessPick`, `ReviewIntegrate`, `ReviewMemoryBootstrapRunning`, `ReviewMemoryCompactRunning/Review` | `V`/diff entry from dashboard or session view | `app/review/{preparation,progression,comments,headless,state}.rs`, `app/review_destination.rs`, `app/review_memory.rs`, `diff.rs`, `diff_split.rs` | Clean by this grep | Planning and review |
| PR Triage | `PrNumberPrompt`, `PrPicker`, `PrReviewLoading`, `PrReview`, `PrInvestigationLoading` | Dashboard PR entry (`normal.rs`) | `app/pr_review/{domain,fetch,actions,reply,investigation,integration,memory,state,runtime}.rs`, `github.rs` | `actions.rs`, `investigation.rs`, `memory.rs`, `reply.rs` **Coupled**; `domain.rs`, `fetch.rs`, `integration.rs` clean by this grep | Planning and review |
| AI PR review / batched review | `AiReview`, `AiReviewRunning` | Dashboard `W` | `app/ai_review.rs`, `review_batch.rs`, `diff_split.rs`, `headless.rs` | `ai_review.rs` **Coupled** | Planning and review |
| Diff-review comment prompt | `DiffReviewPrompt` | Inline from `DiffViewer` | `handlers/diff_review.rs` (no dedicated `app/` module; logic lives in the handler) | N/A — handler-owned, needs extraction before GUI reuse | Planning and review |
| Prompt library (inject a saved prompt into a session) | `PromptLibrary`, `PlaceholderFill` | Leader `P` in an embedded session, or dashboard `L` | `app/prompt_library.rs` | **Coupled** | Settings and libraries |
| Editable prompt overrides manager | `PromptOverrides`, `PromptEditor`, `PromptPrecall` | Dashboard `E` / leader `E` | `app/prompt_overrides.rs`, `app/precall.rs`, `prompts/{mod,defaults,resolve,project}.rs`, `db/prompt_overrides.rs` | Clean by this grep | Settings and libraries |
| Harness setup / config wizard / project agent config | `HarnessSetup`, `ConfigWizard` | First-run and dashboard config entry | `app/config_wizard.rs`, `setup.rs` | Clean by this grep | Settings and libraries |
| Theme / syntax pickers | `ThemePicker`, `SyntaxLanguagePicker` | Dashboard config entry | `theme.rs`, `syntax.rs` | Clean by this grep | Settings and libraries |
| Hooks (lifecycle scripts) | `HookPrompt`, `RunningHook` | `on_start`/`on_stop`/`on_worktree_created` automatic triggers | `app/hooks.rs`, `hook_payload.rs` | Clean by this grep | Settings and libraries |
| Context settings / token tracking display | `ContextSettings` | Dashboard/session context entry | `app/context_settings.rs`, `context_hints.rs`, `context_tracking.rs`, `context_display.rs`, `context_collectors.rs`, `token_tracking.rs` | Clean by this grep | Settings and libraries |
| Feature/session fork | `ForkingFeature` | Dashboard fork action | `handlers/fork.rs` (no dedicated `app/` module found) | N/A — handler-owned | Workspace navigation |
| Batch feature creation | `CreatingBatchFeatures` | Automation-adjacent dashboard flow | `handlers/batch_creation.rs`, `automation.rs` (`CreateBatchFeaturesRequest`) | N/A — handler-owned | Workspace navigation |
| Search | `Searching` | `/` | `app/search.rs` | **Coupled** | Transient dialogs |
| Command picker / skill picker | `CommandPicker`, `SkillPicker` | Leader command palette | `app/commands.rs`, `app/skill_picker.rs` | Clean by this grep | Transient dialogs |
| Bookmarks | `BookmarkPicker` | Dashboard bookmark entry | `app/bookmarks.rs` | Clean by this grep | Transient dialogs |
| Notifications / attention | `NotificationPicker` | Notification indicator | `app/notifications.rs`, `app/attention.rs` | `attention.rs` **Coupled** | Transient dialogs |
| Resource gate / dormancy | `ConfirmResourceStart`, `Dormant` | Launch preconditions; `z` | `app/resource_gate.rs`, `app/dormant.rs`, `resources/{mem,limits,procs,doctor}.rs` | Clean by this grep | Transient dialogs |
| Editors (VS Code launch tracking) | (no dedicated `AppMode`; surfaced through dormancy/stop flows) | Session-open editor action | `app/editor_ops.rs`, `db/editors.rs` | Clean by this grep | Transient dialogs / Session workspace |
| Debug log viewer | `DebugLog` | Dashboard `D` | `app/mod.rs` logging accessors, `debug.rs` | N/A — `mod.rs` is the shared-App file itself, broadly coupled | Settings and libraries |
| Help | `Help` | `?` | `app/mod.rs` (static content) + `help.rs` rendering | N/A | Transient dialogs |
| Markdown viewer / file picker | `MarkdownViewer`, `MarkdownLoading`, `MarkdownFilePicker` | Plan/doc viewing entries | `markdown.rs` | Clean by this grep | Planning and review |
| Path browsing (file explorer) | `BrowsingPath` | Various "attach a file" flows | `handlers/browse.rs`, `ratatui_explorer` (third-party) | N/A — the explorer widget itself is ratatui-native; needs a GUI-native file picker, not a port | Transient dialogs |
| Automation (headless project/feature creation) | N/A — no `AppMode`; IPC-driven | `amf automation create-project/create-feature/create-batch-features` | `automation.rs`, applied inside `app/mod.rs`/`project_ops.rs`/`feature_ops.rs` | Mixed (see above) | N/A — automation is a peer of both UIs, not a GUI destination |

Terminal-pane rendering itself (`vt100` parse + `ratatui` paint of the
embedded session view) is not one `AppMode` — it is the `Viewing` mode's
paint path plus `app/mod.rs`/`app/state.rs`'s pane/vt100 fields, which is
exactly the coupling `AMF_PLAN.md`'s Architecture section already flags and
Task 6 ("Implement the GUI terminal transport") owns. It is listed here for
completeness, not re-scoped.

## Reading the coupling column

12 of the ~70 non-test files under `src/app/` (including `app/mod.rs` and
`app/state.rs`, the two largest and most central files) import
ratatui/vt100/crossterm types directly. That is a minority of files, but it
includes the App root and its top-level state — so "most files are clean"
does not mean the GUI can call into `App` without an adapter layer; it means
the coupling is concentrated enough to extract incrementally rather than
requiring a wholesale rewrite, matching what `AMF_PLAN.md` already concluded.
Three areas (`handlers/diff_review.rs`, `handlers/fork.rs`,
`handlers/batch_creation.rs`) have **no** dedicated `app/` module at all —
their logic lives directly in the handler and needs a extraction pass before
either UI's shared operation exists to call.

## First-slice gate

Per `AMF_PLAN.md`'s Task 1 and Task 7, the vertical slice is **done** when
all of the following hold, exercised against a real (non-mocked) tmux
backend:

1. A user can create a project (pointing at an existing repo) from the GUI.
2. A user can create a feature (with a harness/mode selection) on that
   project from the GUI.
3. The GUI can start that feature's agent session.
4. The GUI shows a live, interactive xterm.js pane attached to that
   session — input, output, resize, and scrollback all work.
5. Closing the GUI window does not stop the agent session (tmux keeps
   running).
6. Reopening the GUI reconnects to the same project/feature/session without
   creating a duplicate session or duplicate worktree, and the pane
   reattaches with correct history instead of a blank/replayed screen.
7. Every step above also works unmodified from the **TUI** on the same
   database — i.e., a feature created in the GUI is visible and startable
   from `amf`, and vice versa, with no schema migration or manual fixup
   required.

Explicitly out of scope for the gate: dashboard styling, any workflow beyond
create/start/attach/reconnect, and concurrent GUI+TUI operation on the same
feature at the same time (that is Task 5's "cross-process coordination,"
still an open question per `AMF_PLAN.md`'s risks section).

## TUI regression policy (applies at every milestone in `AMF_PLAN.md`'s Tasks section)

Before a milestone is marked complete, run and pass, unmodified from
`docs/development/checks.md`:

```sh
cargo build --locked
cargo test --workspace --locked
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Additionally:

- Diff `cargo test --workspace --locked -- --list` against the pre-milestone
  baseline; a shrinking test count on an unrelated suite is a regression
  signal, not just a coverage gap.
- For any milestone touching a file in the "Coupled" column above, manually
  smoke-test that feature's TUI flow (its `AppMode` is still reachable via
  its documented key, and its dialog still renders) — those files are the
  ones most likely to be perturbed while extracting a shared operation.
- For milestones touching `app/mod.rs`/`app/state.rs` (Tasks 2–5), also run
  the full suite on macOS if available, since those two files carry the
  bulk of the platform-sensitive pane/vt100 state.

No milestone in this feature removes, mocks out, or skips an existing TUI
test to make room for GUI code, per `AMF_PLAN.md`'s "additive" decision.
