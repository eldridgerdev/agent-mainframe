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
| Available | Project and feature creation, feature start/stop, feature deletion (tmux session, worktree and record, with the TUI's unfinished-TODO disposition), additional allowed-agent, terminal and Neovim sessions with optional names, stopping, starting and closing a single session (a stopped session stays listed and stays stopped when its feature starts; starting an agent session offers to resume, pick or clear its saved conversation), session navigation and live tmux terminal attachment/reconnection. |
| Limited | VS Code and configured custom sessions still require the TUI session picker. |
| Available | Local prompt composer on Claude, Codex, OpenCode and Pi tabs. Enter adds a line; Ctrl/Cmd+Enter or Send prompt submits the complete draft through tmux bracketed paste. Drafts stay per session through navigation and refresh while the window is open, failed sends retain text, and duplicate sends are blocked. TODO, planning and Learning handoffs use the same composer. Shell/editor input stays direct. |
| Available | Global, project and worktree TODO lists: add, change status, delete, reorder, move and copy. Start a TODO agent in an existing feature or a new git worktree feature with an editable, unsent prompt. |
| Available | Full and Quick Plan on an existing feature or while creating a feature; Full Plan for a TODO in an existing feature or a new git worktree. Review/edit, headless-call notice, cancellation and explicit approval for over-limit starts. |
| Available | Script-only and choice-prompting `on_worktree_created` hooks during ordinary creation, direct TODO creation, Full/Quick creation-time planning and TODO planning. The creation forms load project-specific options before submission; missing or invalid choices are rejected before creating a checkout or reserving a TODO. Hooks finish before the interview opens; cancelling that interview keeps the already-created worktree without launching its agent. |
| Available | Deleting a feature that hosts the project TODO list asks which surviving feature should keep it, or whether to delete the list and its TODOs. The GUI collects this choice before deletion; Cancel leaves the feature and both lists untouched. |
| Available | Learning on a feature: repository file tree or branch changes, file/line/hunk/project questions, starter questions, persisted answers, follow-ups, deep dives, intent relabelling, keep-as-TODO with editable title and notes, harness and reading-level selection, anchor-drift notices, and explicit editing-agent handoff with resource approval and an unsent draft. A stopped linked editing session is restarted through its tab's resume choice rather than duplicated. |
| Limited | Native desktop Learning interactions and paid-harness runs are not yet validated. |
| Available | Standalone diffs on Git features, including stopped features: all current changes or one feature commit, file filtering, hunk navigation, unified and side-by-side layouts, whitespace filtering, automatic or chosen base ref, expanded context, rename/mode metadata, binary notices and explicit refresh. |
| Limited | Native WSLg captures verify the stopped-feature entry, current changes, unified/split layouts, whole-file context, commit selection and binary notice through real Rust IPC. Other native interactions and macOS validation remain open. Standalone Changes is read-only; use Final Review for whole-file and line/range comments and suggestion editing. Walkthroughs, AI questions and feedback handoff remain in the TUI. |
| Available | Manual Final Review on Git features, including stopped features: approve/reject/skip, undo verdicts, whole-file and line/range comments with severity, suggestion editing and confirmed local application, resolve/reopen comments, overall feedback, developer notes, saved line threads/suggestions, persisted application history, read-only review round history with on-demand archive loading, a complete pre-finish summary with explicitly confirmed batch suggestion preparation, and pause/resume using the TUI progress file. Local application uses the shared TUI write guards, consumes the replacement, resolves its thread, refreshes the diff and invalidates the changed file's approval/undo. Changed patches and detected external progress edits require refresh/reload; failed saves retain edits with retry. |
| Available | Final Review per-file Claude walkthroughs, cached changeset overview, Claude co-review drafts with explicit accept/dismiss, and repository-aware questions/follow-ups using the project's allowed Claude, Codex, OpenCode or Pi harnesses. Each new AI call requires a pre-call confirmation with prompt preview; polling never launches work. Changed patches, feature targets and external review saves discard pending results. Question conversations and generated notes remain in memory for the open review; accepted co-review findings share the TUI's saved progress. Answered questions can draft inline comments or overall feedback with their answering harness, another pre-call notice and editable text. Transfer checks the repository again without an AI call, appends existing prose and opens an unsaved editor; explicit Save keeps the feedback. |
| Limited | Final Review checks, completing the review and feedback dispatch remain in the TUI. GUI batch preparation keeps the review open to inspect changed source; automated coverage is available, with native desktop validation still open. Seven asserted native WSLg frames verify round history, archive loading, draft restoration and read-error recovery through real Rust IPC; remaining interactions and macOS validation stay open. Question-to-comment drafting has command/component tests and eight asserted native WSLg frames through real Rust IPC with offline Codex; paid-harness execution remains unvalidated. Automated tests and eight asserted native WSLg frames cover walkthroughs, questions and co-review using offline CLI fixtures and real Rust IPC. Paid-harness runs remain unvalidated. Native WSLg captures verify entry, notes/threads, verdicts/comments, split layout, unsaved-draft protection, pause/resume, changed-patch rejection and refresh invalidation through real Rust IPC. Native WSLg captures also verify range selection, line/range prose and replacement editing, split display, unsaved-suggestion protection, thread resolution/reopening and pause/reopen persistence. Native WSLg captures also verify confirmed local application and cancellation, changed source and approval invalidation, stale-file and read-only-file refusals, progress-save retry and restored application history. Remaining native interactions and macOS validation are open. Edit a given review in one interface at a time. |
| Planned | Supervised edits and PR triage, as part of the remaining diff-related parity work. |
| Planned | Code syntax highlighting in GUI diffs, reviews and the Learning source reader, after the remaining diff-related parity work and before the remaining GUI parity work. |
| Available | Prompt library browsing from workspace navigation or an agent composer, with shared user/global/project/worktree sources, fuzzy name/body and `#tag` search, original/resolved previews, required text/multiline fields and configured/inline choices. Add to draft targets an allowed Claude, Codex, OpenCode or Pi session, appends existing text and leaves sending explicit. Stopped agent drafts are editable before start. Template/checkout changes and stale/deleted/disallowed targets are rechecked before insertion. |
| Limited | Prompt-library native desktop and macOS interactions remain unvalidated. Command/component regressions cover cancellation, delayed responses, duplicate insertion, external edits and draft preservation. Template authoring, deletion/export and editable prompt overrides remain in the TUI. |
| Planned | Settings and the other workflows listed below, after code syntax highlighting. |

Native GUI diff proof is reproducible with
[`gui-standalone-diffs.txt`](../../scripts/dev/screenshot/scenarios/gui-standalone-diffs.txt).
It uses an isolated Git checkout/database and starts no agent. The same scenario
runs through the repository's private screenshot publisher for PR review.

Native manual-review proof is reproducible with
[`gui-manual-review.txt`](../../scripts/dev/screenshot/scenarios/gui-manual-review.txt).
It uses the same isolated GUI fixture and starts no agent.

Native line/range comment and suggestion-editing proof is reproducible with
[`gui-review-line-comments.txt`](../../scripts/dev/screenshot/scenarios/gui-review-line-comments.txt).
Its seven frames check GUI states and shared progress anchors through real Rust
IPC, while asserting that authoring suggestions leaves the source file unchanged.

Native local suggestion-application proof is reproducible with
[`gui-review-local-suggestions.txt`](../../scripts/dev/screenshot/scenarios/gui-review-local-suggestions.txt).
Its seven frames exercise confirmation and cancellation, successful source writes,
approval invalidation, stale-source and read-only-source refusals, progress-save
retry, and persisted application history after pause/reopen. The helper asserts
both GUI states and the isolated checkout/progress files; no agent is launched.

Native walkthrough, question and co-review proof is reproducible with
[`gui-review-ai.txt`](../../scripts/dev/screenshot/scenarios/gui-review-ai.txt).
Eight frames check prompt previews, generated notes, local questions and answers,
co-review drafts, accepted-finding persistence and changed-patch refusal through
real Rust IPC. Offline CLI fixtures and invocation-log assertions verify that
opening, editing, cancelling and reopening never launch extra calls.

Native question-to-comment proof is reproducible with
[`gui-review-question-drafts.txt`](../../scripts/dev/screenshot/scenarios/gui-review-question-drafts.txt).
Eight frames check the new drafting actions, explicit call confirmation and
cancellation, editable generated text, unsaved-draft protection, inline transfer
with preserved thread range/severity/suggestion, overall feedback and pause/reopen
persistence. Offline Codex invocation logs and checkout/progress assertions prove
that transfer runs no AI call, saves no feedback and leaves source untouched.

Worktree-hook choices have automated coverage for creation, TODO launches,
Full/Quick planning, lookup retry, and cancellation. Native desktop interaction
checks for these choices remain open.

The Linux development build and automated suites pass, and the GUI has been
run under WSL2/WSLg during development, which is where attaching a terminal
from a desktop launch (no terminal on stdin) was fixed. The macOS app has not
been run by hand yet. Releases publish x86_64 and aarch64 Linux `.deb`
and AppImage builds and an ad-hoc-signed Apple Silicon `.dmg`; Developer ID
signing and notarization are still open. The TUI remains
the complete interface for workflows marked Planned.

## Remaining GUI parity priority

Completed increments and remaining work, in priority order:

- [x] **Agent prompt composer (user-requested priority, 2026-10-04).** All four
  agent tabs offer local drafting and explicit submission. Drafts belong to
  sessions and survive navigation and refresh while the GUI window stays open.
  Automated interactions cover Unicode/multiline text, send failures, duplicate
  prevention, attachment readiness and workflow seeds. Four asserted native
  GUI frames verify drafting without sending, independent Claude/Codex drafts,
  restoration after tab switching and exactly one complete message delivered
  through real tmux IPC. Paid-harness runs and macOS validation remain open.
  This explicit request takes priority over the general parity ordering below.

  Native proof is reproducible with
  [`gui-agent-composer.txt`](../../scripts/dev/screenshot/scenarios/gui-agent-composer.txt).
  It preserves HOME and uses a private database and tmux server with lightweight
  harness fixtures; no paid agents or real AMF sessions are started.

- [x] **Final Review line/range comments and suggestion editing.** Select lines
  in unified or split diffs, edit prose and replacement code, resolve/reopen
  kept threads, and pause/resume their shared progress. Command/component tests
  and seven asserted native WSLg frames cover this increment.

- [x] **Prompt library (user-requested priority, 2026-10-05).** Browse/search the
  shared library, preview a template, fill text placeholders and configured
  select choices, then target an allowed agent session and insert the resolved
  prompt into its unsent composer for explicit sending. Reuses the TUI library,
  template resolution and persistence while retaining existing drafts.
  Implemented immediately after review history, ahead of the remaining
  diff/review work and syntax highlighting. Editable prompt overrides remain a
  separate later item.

  Implemented the scoped two-pane browser and explicit unsent composer handoff.
  Seven Rust regressions and thirteen new frontend interactions cover source
  merging, anonymous config identity, defaults/required fields/choices, external
  template edits, target restrictions/deletion, delayed completions, cancellation,
  duplicate insertion and all four agent composers. The normal parallel workspace
  suite passes (3,235 library tests and seven GUI-crate tests; one pre-existing
  live-GitHub acceptance test ignored). All 136 frontend tests, production frontend
  build, both executable builds, formatting and strict workspace/all-target Clippy
  pass. The library inventory retains all 3,229 baseline test names and adds seven. Native desktop and macOS
  interaction validation remain open; authoring/export and overrides are later work.

- [ ] **Remaining diff and review parity.** Complete diff-related workflows,
  including finish-time batch suggestion application, finish checks and feedback
  handoff, plus supervised edits and PR triage.
  - [x] **Pre-finish summary and batch preparation (2026-10-05).** The GUI
    projects every verdict and open kept thread from the shared TUI summary,
    retaining drafts and save errors without progress writes. Explicitly
    confirmed batch preparation shares the TUI finish-time source-write step,
    consumes its opt-in, settles successful jobs, reports blocked suggestions,
    invalidates changed approvals and retains source writes after save failure.
    Stale revisions, changesets, progress and feature targets are refused.
    Six new Rust regressions and six frontend interactions pass. The full normal
    parallel workspace suite passes (3,242 library and seven GUI-crate tests;
    one existing live-GitHub test ignored), along with all 144 frontend tests,
    the production frontend build, both executable builds, formatting and strict
    workspace/all-target Clippy. Finish checks, completing the review and
    explicit feedback handoff are next. Native desktop and macOS validation
    remain open.
  - [x] **Local suggestion application (2026-10-04).** Explicit confirmation
    writes one exact kept thread through the TUI engine, settles it, refreshes
    source and persists the application record. Stale targets/patches/progress,
    inapplicable spans, duplicate requests, source-write failure and progress-save
    retry have automated coverage and seven asserted native WSLg frames. Remaining native interactions and macOS validation stay open.
  - [x] **Walkthroughs and AI questions/co-review (2026-10-04).** Shared engines power explicit GUI AI requests, prompt previews/confirmation/cancellation, workflow-scoped completion polling, all four question harnesses and exact co-review draft acceptance/dismissal. Local question drafts survive failed/cancelled calls. Saved AMF review artifacts no longer invalidate repository-aware questions in either interface. Eleven new Rust regressions and eleven new frontend interactions pass, together with the full parallel workspace suite, all 106 frontend tests, both builds, formatting and strict Clippy. All 3,207 baseline test names remain present. Eight asserted native WSLg frames verify the AI controls, prompt preview, generated output, local multiline question and answer, saved-finding persistence and changed-patch refusal through real Rust IPC with offline harnesses. The scenario is [`gui-review-ai.txt`](../../scripts/dev/screenshot/scenarios/gui-review-ai.txt); invocation-log checks prove cancellation/reopen never start extra calls. Paid-harness runs, remaining native interactions and macOS runtime validation remain open. Question-to-comment drafting is implemented in the following increment.
  - [x] **AI-assisted question-to-comment drafting (2026-10-05).** Draft from an answered question into an inline thread or overall feedback using its answering harness and shared prompt confirmation. Edit locally, transfer with a fresh repository stamp check and no AI call, then save explicitly through the existing comment engine. Existing prose, full containing spans, severity and suggestions survive; ambiguous or partial overlaps are refused. Eight new Rust regressions and seven frontend interactions cover failure/cancellation/staleness, containing-thread preservation in GUI/TUI, and unsaved text (including clearing existing prose). The full parallel Rust suite passes (3,219 library and seven GUI-crate tests; one pre-existing live-GitHub test ignored), along with all 117 frontend tests, the production frontend build, formatting, strict workspace/all-target Clippy and both executable builds. All 19 focused GUI review AI tests pass after the final edit-retention refinement. Eight asserted native WSLg frames also verify the new actions, pre-call confirmation/cancellation, edited generated drafts, unsaved discard protection, preserved range/severity/suggestion on inline transfer, explicit save, overall transfer and pause/reopen persistence. Invocation-log and progress/source assertions prove transfer runs no AI call, saves no feedback and leaves source untouched. Representative PNGs were visually inspected. Reproduce with [`gui-review-question-drafts.txt`](../../scripts/dev/screenshot/scenarios/gui-review-question-drafts.txt). Paid-harness runs, remaining native interactions and macOS runtime validation stay open. Review history is completed in the following increment; finish workflows follow.
  - [x] **Review history (2026-10-05).** Browse Current and completed rounds newest first with original feedback, suggestions, checks and agent replies. Explicitly load older archived rounds. Share the TUI loader and current-review projection; preserve local comment/question/AI drafts and outstanding save errors without progress writes. Six Rust regressions and six frontend interactions cover lazy loading, ordering, read failures, stale/deleted targets, draft retention and TUI tail navigation. Returning to the editor stays available after feature deletion. The final normal parallel workspace suite passes (3,227 library and seven GUI-crate tests; one pre-existing live-GitHub acceptance test ignored), as do all 123 frontend tests, the production frontend build, both executable builds, formatting, strict workspace/all-target Clippy and whitespace checks. All 3,229 baseline test names remain present, with six additions. Seven asserted native WSLg frames now verify the history entry with an unsaved draft, Current state, completed-round check output/suggestions/agent replies, newest-first archive loading, restored multiline editor text, archive read failure and retry after repair through real Rust IPC. Source and saved-progress assertions confirm browsing writes neither. Representative PNGs were visually inspected. The capture preserves HOME, uses private SQLite/Git fixtures and a private tmux socket, launches no agents, and cleans up its GUI/Vite processes. Reproduce with [`gui-review-history.txt`](../../scripts/dev/screenshot/scenarios/gui-review-history.txt); local proof is `/tmp/amf-gui-review-history-proof`. Remaining native interactions and macOS validation stay open. Next: the user-prioritized prompt library, then the remaining finish workflows.
- [ ] **Code syntax highlighting.** Highlight source code in unified and
  side-by-side diffs, review views and the Learning source reader. Keep change
  markers and line numbers readable, with plain-text fallback for unsupported
  languages. Schedule this after all remaining diff-related parity items and
  before the remaining GUI parity work.
- [ ] **Other GUI parity.** Continue prompt overrides, dormancy, settings,
  VS Code/custom sessions and the remaining workflow inventory.

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
