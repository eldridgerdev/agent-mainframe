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
| Available | Project and feature creation, feature start/stop, feature deletion (tmux session, worktree and record, with the TUI's unfinished-TODO disposition), additional allowed-agent, terminal and Neovim sessions with optional names, the rest of the TUI `s` picker (VS Code, the per-feature TODOs session and the effective `amf.json` custom sessions with icon, description, command, working directory, pre-check, on-stop and autolaunch), stopping, starting and closing a single session (a stopped session stays listed and stays stopped when its feature starts; starting an agent session offers to resume, pick or clear its saved conversation), session navigation and live tmux terminal attachment/reconnection. |
| Available | VS Code uses the TUI picker's launch (`code --new-window`, a tracked `launched_editors` row and background window attribution), so feature stop and dormancy cleanup apply unchanged. It has no tmux pane: a **VS Code** tab and a sidebar chip list the feature's tracked windows (opening, AMF's, not AMF's) and close AMF's after confirmation through the shared editor cleanup (the TUI dormant list's `e`). A window opened after the list loaded is refused rather than closed unseen. Custom sessions run their `pre_check` without holding the GUI lock, show a failure with its output in the dialog, and refuse an entry changed or removed in `amf.json` since the dialog listed it. Duplicate custom adds are refused while their pre-check is running. A non-autolaunch session is added without switching tabs, as the TUI stays on its dashboard. Like every GUI add, a session that would start a stopped feature asks for resource approval (the TUI only warns for custom sessions). An unreadable project `amf.json` is reported rather than silently treated as empty. Six asserted native WSLg frames cover the picker, the pre-check failure, custom-session attachment and the VS Code open/close flow with stand-in `code`, `docker` and `npm` executables. |
| Limited | VS Code has no terminal pane. Focusing an existing external window remains open; **Open another VS Code window** always uses the tracked new-window launch. Native macOS validation of VS Code and custom sessions remains open. |
| Available | Sidebar parity with the TUI dashboard tree: shortened project paths, empty-project and paused creation-plan hints; feature status glyphs in the TUI's precedence, nickname/branch, `repo`, issue source, open/merged/closed PR badges with open threads, token usage, mode/review/plan/plan-paused badges, age, session and stopped-session counts, the `?` request marker, workdir and AI summary; nested session rows with kind icons, run state, agent context indicator and status line that open their tab; collapse state persisted in the shared `collapsed` flags. Context, usage, status text and open PRs come from the TUI's own collectors running in the GUI process; merged/closed PRs, thinking markers and waiting notification files are read from their shared on-disk forms. |
| Deliberate GUI difference | Collapsed feature rows hide the mode badge (`vibeless`, `vibe`, `supervibe`); expanding restores it. The mode remains in the feature tooltip in both states. The TUI continues showing the mode on every row. Other badges remain visible. Frontend regressions cover all three modes and default collapse state; two asserted native frames verify collapsed/expanded rows and the tooltip through the isolated `gui-collapsed-mode-badge.txt` scenario. |
| Limited | The sidebar omits TUI-process state with no shared form: the attention reason, TUI background hooks, TUI deletion, summary generation and AI-review progress. `remote` and the pending worktree script render when present but are not persisted, so another process's values never reach the GUI. While a TUI owns the IPC socket, thinking and waiting can be under-reported. OpenCode thinking uses the sidebar cache only. Six asserted native WSLg frames cover the tree, session rows, collapse round trip and paused-plan resume; macOS validation remains open. |
| Available | Local prompt composer on Claude, Codex, OpenCode and Pi tabs. Enter adds a line; Ctrl/Cmd+Enter or Send prompt submits the complete draft through tmux bracketed paste. Drafts stay per session through navigation and refresh while the window is open, failed sends retain text, and duplicate sends are blocked. TODO, planning and Learning handoffs use the same composer. Shell/editor input stays direct. |
| Available | Terminal scroll-back in agent, shell and editor tabs. Wheel, trackpad, Shift+PageUp/PageDown/Home/End, and PageUp/PageDown/Home/End/Esc while reading. Normal-screen panes load the TUI scroll mode's shared tmux history snapshot (`capture_scrollback`, 10,000 lines) into xterm's scrollback. Live output is held while reading, a "New output below" banner appears, and the reader's position stays put. Jump to latest, Esc, Shift+End, scrolling to the bottom or typing resumes live. Nothing is sent to the pane and tmux copy-mode is never used. Full-screen programs that requested mouse reporting (OpenCode, Neovim) receive wheel reports after the pane's modes are re-read. |
| Limited | Full-screen programs without mouse reporting (`less`, some TUIs) show a notice; use their own keys. The history view is a snapshot, not a stream. Plain PageUp/Home/End/Esc still reach the program from the live view. Native WSLg frames use offline fixtures. Real-harness and macOS validation remain open. |
| Available | Global, project and worktree TODO lists: add, change status, delete, reorder, move and copy. Start a TODO agent in an existing feature or a new git worktree feature with an editable, unsent prompt. |
| Available | Full and Quick Plan on an existing feature or while creating a feature; Full Plan for a TODO in an existing feature or a new git worktree. Review/edit, headless-call notice, cancellation and explicit approval for over-limit starts. |
| Available | Script-only and choice-prompting `on_worktree_created` hooks during ordinary creation, direct TODO creation, Full/Quick creation-time planning and TODO planning. The creation forms load project-specific options before submission; missing or invalid choices are rejected before creating a checkout or reserving a TODO. Hooks finish before the interview opens; cancelling that interview keeps the already-created worktree without launching its agent. |
| Available | Deleting a feature that hosts the project TODO list asks which surviving feature should keep it, or whether to delete the list and its TODOs. The GUI collects this choice before deletion; Cancel leaves the feature and both lists untouched. |
| Available | Learning on a feature: repository file tree or branch changes, file/line/hunk/project questions, starter questions, persisted answers, follow-ups, deep dives, intent relabelling, keep-as-TODO with editable title and notes, harness and reading-level selection, anchor-drift notices, and explicit editing-agent handoff with resource approval and an unsent draft. A stopped linked editing session is restarted through its tab's resume choice rather than duplicated. |
| Limited | Native desktop Learning interactions and paid-harness runs are not yet validated. |
| Available | Standalone diffs on Git features, including stopped features: all current changes or one feature commit, file filtering, hunk navigation, unified and side-by-side layouts, whitespace filtering, automatic or chosen base ref, expanded context, rename/mode metadata, binary notices and explicit refresh. |
| Limited | Native WSLg captures verify the stopped-feature entry, current changes, unified/split layouts, whole-file context, commit selection and binary notice through real Rust IPC. Other native interactions and macOS validation remain open. Standalone Changes is read-only; use Final Review for whole-file and line/range comments and suggestion editing. Walkthroughs, AI questions and feedback handoff remain in the TUI. |
| Available | Manual Final Review on Git features, including stopped features: approve/reject/skip, undo verdicts, whole-file and line/range comments with severity, suggestion editing and confirmed local application, resolve/reopen comments, overall feedback, developer notes, saved line threads/suggestions, persisted application history, read-only review round history with on-demand archive loading, a complete pre-finish summary with explicitly confirmed batch suggestion preparation, confirmed project checks with bounded output, cancellation and stale-result handling, and pause/resume using the TUI progress file. Local application uses the shared TUI write guards, consumes the replacement, resolves its thread, refreshes the diff and invalidates the changed file's approval/undo. Changed patches and detected external progress edits require refresh/reload; failed saves retain edits with retry. |
| Available | Final Review per-file Claude walkthroughs, cached changeset overview, Claude co-review drafts with explicit accept/dismiss, and repository-aware questions/follow-ups using the project's allowed Claude, Codex, OpenCode or Pi harnesses. Each new AI call requires a pre-call confirmation with prompt preview; polling never launches work. Changed patches, feature targets and external review saves discard pending results. Question conversations and generated notes remain in memory for the open review; accepted co-review findings share the TUI's saved progress. Answered questions can draft inline comments or overall feedback with their answering harness, another pre-call notice and editable text. Transfer checks the repository again without an AI call, appends existing prose and opens an unsaved editor; explicit Save keeps the feedback. |
| Available | Completing Final Review from the pre-finish summary with explicit confirmation: the saved apply-on-finish batch, a rerun of the configured check (cancellable; earlier summary results are not reused) and the TUI's round recording, progress clearing and optional PR post. Actionable feedback can be handed to the feature's first agent session, submitted when the TUI setting submits and the session runs, otherwise as an unsent composer draft, or completion can skip the handoff. Stale revisions, patches, saved progress, check commands, agent sessions and deleted/moved features are refused before writing; completion is delivered once and unsaved/generated drafts block it. |
| Limited | Seven asserted native WSLg frames verify completion entry, unsaved-draft blocking, the confirmation, changed-patch refusal, the running/cancelled completion check and the unsent-draft handoff through real Rust IPC with no agent receiving anything. Choosing another feedback destination (dedicated session, another feature, companion feature) remains in the TUI; paid-agent delivery, remaining native interactions and macOS validation stay open. Desktop check results stay in the open review; completion reruns the configured check. Seven asserted native WSLg frames verify check previews, confirmation, failed/passing output, cancellation and stale-result rejection through real Rust IPC. Remaining native interactions and macOS validation stay open. GUI batch preparation keeps the review open to inspect changed source; automated coverage is available, with native desktop validation still open. Seven asserted native WSLg frames verify round history, archive loading, draft restoration and read-error recovery through real Rust IPC; remaining interactions and macOS validation stay open. Question-to-comment drafting has command/component tests and eight asserted native WSLg frames through real Rust IPC with offline Codex; paid-harness execution remains unvalidated. Automated tests and eight asserted native WSLg frames cover walkthroughs, questions and co-review using offline CLI fixtures and real Rust IPC. Paid-harness runs remain unvalidated. Native WSLg captures verify entry, notes/threads, verdicts/comments, split layout, unsaved-draft protection, pause/resume, changed-patch rejection and refresh invalidation through real Rust IPC. Native WSLg captures also verify range selection, line/range prose and replacement editing, split display, unsaved-suggestion protection, thread resolution/reopening and pause/reopen persistence. Native WSLg captures also verify confirmed local application and cancellation, changed source and approval invalidation, stale-file and read-only-file refusals, progress-save retry and restored application history. Remaining native interactions and macOS validation are open. Edit a given review in one interface at a time. |
| Available | Supervised edits (Vibeless per-edit approval) from the hook's file fallback, which a Claude or OpenCode hook uses when no AMF TUI is listening: navigation and feature-page badges with an arrival notice, an automatic oldest-first popup over the current page or agent tab with the configured answer hold, dismissal without answering and focus restoration, deferral around dialogs and unsent form/agent drafts, a GUI-only auto-open preference, the hook's captured diff in unified/side-by-side layouts with adjustable context, the agent's stated reason, and explicitly confirmed approve, reject-with-feedback and cancel answers that state each harness's effect. Answers name a stable edit ID and the reviewed revision; changed, already-answered and abandoned edits are refused, and unsent feedback requires explicit discard. |
| Limited | Edits delivered over IPC to a running TUI are answered in that TUI; the GUI binds no IPC socket. AI explanations of an edit remain TUI-only. Codex Vibeless review is unsupported in both interfaces. Native WSLg frames cover Claude's hook; OpenCode, the change-reason flow and macOS have automated coverage only. |
| Available | PR Triage on Git features: open-or-all pull request list with the branch's PR marked, opening by number, the shared comment fetch/cache (inline comments with windowed diff context, review summaries, conversation/bot comments, thread resolution and collated AMF replies), hide-resolved and sort controls, local done/skip, existing feature-agent selection and editable fix prompts handed once to an unsent composer draft with existing text preserved, or explicitly submitted to a running agent after an exact-prompt confirmation (with Fixing persistence and correlated agent reply drafts), read-only investigations and follow-ups with the shared pre-call notice and exact-prompt preview, dismissal, Done/Not needed/findings replies with an exact posted-body confirmation, and confirmed thread resolve/reopen. GitHub writes re-read the PR head or thread first and refuse stale targets while keeping the draft. |
| Limited | PR Triage dedicated/companion fix destinations, batch fixes, integration, review memory, AI Review and keep-as-TODO remain in the TUI. Existing feature agents can receive unsent fix drafts through the GUI; PR head and session identity are rechecked and edited drafts survive refusals. Seven asserted native WSLg frames use an offline `gh` that refuses writes and offline harness fixtures; real GitHub writes, paid-harness runs and macOS validation remain unvalidated. |
| Available | Code syntax highlighting in unified/side-by-side diffs, Final Review, supervised edits and the Learning reader, using the TUI's tree-sitter service: the same language detection, parsers and token classes, with whole-file context for multi-line strings and comments. File headers name the language or explain plain text (unknown language, parser missing or broken, too large, binary), and a missing or broken parser can be installed after explicit confirmation with the TUI picker's installer. Colours come from `--syn-*` tokens for light and dark. |
| Limited | The GUI installs only the shown file's parser; uninstalling and the full language list stay in the TUI's syntax picker. Twelve asserted native WSLg frames cover the install, five languages, a review line comment and the Learning reader in light and dark; macOS validation remains open. |
| Available | Prompt library browsing from workspace navigation or an agent composer, with shared user/global/project/worktree sources, fuzzy name/body and `#tag` search, original/resolved previews, required text/multiline fields and configured/inline choices. Add to draft targets an allowed Claude, Codex, OpenCode or Pi session, appends existing text and leaves sending explicit. Stopped agent drafts are editable before start. Template/checkout changes and stale/deleted/disallowed targets are rechecked before insertion. |
| Limited | Prompt-library native desktop and macOS interactions remain unvalidated. Command/component regressions cover cancellation, delayed responses, duplicate insertion, external edits and draft preservation. Template authoring and deletion/export remain in the TUI. |
| Available | Headless prompt override manager from workspace navigation, with every registry prompt, its effective layer (and per-harness winner), placeholders, effective and built-in templates. Save feature, project (`amf.json`) or global overrides, shared or per harness. Clear one stored override with confirmation. Unsaved-edit protection is included. Stale saves after TUI, other-window or hand edits of `amf.json` are refused until reloaded, and malformed `amf.json` is reported without being overwritten. Pending Final Review and planning AI calls link to the manager through **Edit prompt**. |
| Limited | Prompt-override macOS interactions remain unvalidated. A pre-call notice's preview is not re-rendered after an edit; continuing the call re-resolves the saved override. |
| Available | Dormant features: running features idle and unopened past the configured thresholds, with ages and timestamps, and an explicitly confirmed stop of a selection through the shared TUI stop, including tracked-editor cleanup and its closed/left-running/still-opening report. Each feature is re-checked at confirm time; deleted, already stopped, restarted, shared-session, opened, newly active and duplicate selections are refused with a reason. Showing a session in the GUI counts as opening its feature, as in the TUI. |
| Limited | Closing only a dormant feature's editor and deleting from the dormant list remain TUI actions. Four asserted native WSLg frames verify the list, confirmation, a confirm-time refusal and editor ownership through real Rust IPC with stand-in editors; macOS validation is open. |
| Available | Per-session agent sidebar: shared TUI section assembly/order for Claude, Codex, OpenCode and Pi; status tokens/cost/model, usage meters, context bands and estimated/stale labels, plan opening, issue/PR state and Triage, work and supervised-edit Review, summary (last for OpenCode), clamped prompt with View/Reuse into the draft, agent TODO progress and linked AMF TODO completion. Per-viewer collapse preference; terminal resize preserves scrollback and draft. Works without a TUI. Sources and TUI-only fields are recorded individually in `src/gui_contract/session_sidebar.rs`. |
| Limited | Session sidebar omits TUI-only attention reasons, tool-call IPC, Codex live events and other-process worker activity. Context fresh-start remains a TUI action. Projects-sidebar hide/restore and keyboard shortcuts belong to the separate show/hide-sidebars increment. Native offline proof uses `gui-session-sidebar.txt`; macOS validation remains open. |
| Planned | Settings and the other workflows listed below, after code syntax highlighting. |

Native GUI diff proof is reproducible with
[`gui-standalone-diffs.txt`](../../scripts/dev/screenshot/scenarios/gui-standalone-diffs.txt).
It uses an isolated Git checkout/database and starts no agent. The same scenario
runs through the repository's private screenshot publisher for PR review.

Native PR Triage proof is reproducible with
[`gui-pr-triage.txt`](../../scripts/dev/screenshot/scenarios/gui-pr-triage.txt).
Its seven frames cover the PR list, a normalized thread, the investigation
pre-call, an offline Codex answer, the reply confirmation, a stale-head refusal
and the resolve confirmation. The fixture `gh` logs every call and refuses any
write; the capture asserts it recorded none.

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

- [x] **Sidebar parity with the TUI dashboard tree (user-requested, 2026-10-06).**
  The sidebar carries every row, badge and glyph `src/ui/list.rs::draw` shows,
  nested session rows and the shared collapse state. Each runtime signal is
  derived in the GUI process with the TUI's own collector, read from a shared
  on-disk form, or deliberately left out as TUI-process state. Native proof is
  reproducible with
  [`gui-sidebar-parity.txt`](../../scripts/dev/screenshot/scenarios/gui-sidebar-parity.txt),
  which uses a private HOME, database, Git checkouts and tmux server and an
  offline, read-only `gh`.

- [x] **Agent session sidebar (2026-10-07, PR #704).** Shared per-harness
  sections, read-only plan opening, prompt viewing/reuse and confirmed linked-TODO
  completion work without a TUI. Viewer-local hide/restore keeps the terminal,
  scrollback and composer mounted and resizes its pane. Eight asserted offline
  native frames cover Claude, Codex and Pi; OpenCode ordering and all four
  contracts have automated coverage. Project-sidebar controls and keyboard
  shortcuts, dark/custom themes, native OpenCode and macOS proof
  remain open. This completes the agent-sidebar part of show/hide sidebars.

- [x] **GUI fresh context (2026-10-07).** The Context section opens an
  editable shared continuation seed, starts a new session with the feature's
  configured harness and selects its unsent composer draft. Original sessions
  stay open. Cancellation protects edited text; resource warnings require
  explicit approval. Stale/deleted/reassigned sources and repeated submissions
  are refused before launch. Native proof scenario: `gui-fresh-context.txt`.
  Validation: three Rust regressions and five frontend interactions; the full
  parallel workspace suite (3,399 library + 7 GUI tests), all 286 frontend
  tests, production build, formatting and strict workspace/all-target Clippy
  pass. Native proof uses isolated fixtures and sends no agent input.
  macOS runtime validation remains open.

- [x] **PR Triage existing-agent fix drafts (2026-10-08).** Choose an
  existing feature agent, edit the TUI-shared comment fix prompt (including
  investigation findings), then open it once as an unsent composer draft.
  Existing text is preserved and stopped destinations stay stopped. Changed PR
  heads, comments and destination identities are refused; unsent edits survive
  failures and require explicit discard on cancel/close. Completion releases the
  backend workflow before opening the agent tab. Preparing a draft neither marks
  the comment Fixing nor creates an agent reply receipt. Dedicated/companion
  destinations, batch fixes/integration,
  review memory and keep-as-TODO remain open.
  Validation: three Rust regressions and five frontend interactions, full normal
  parallel workspace tests (3,402 library + 7 GUI tests; one pre-existing live
  GitHub test ignored), all 291 frontend tests, production frontend/workspace
  builds, formatting and strict workspace/all-target Clippy pass. An initial
  frontend run hit two existing timing failures under Rust compile contention;
  the normal rerun passed after compilation ended. Six asserted native WSLg
  frames through real IPC use private Git/SQLite/tmux and offline gh/harnesses,
  covering target selection, investigation context, draft protection, stale-head
  refusal and a once-only handoff preserving an existing draft. Representative
  PNGs were visually inspected. Reproduce with
  [`gui-pr-fix-drafts.txt`](../../scripts/dev/screenshot/scenarios/gui-pr-fix-drafts.txt).
  Paid-agent sending and macOS runtime remain unvalidated.

- [x] **PR Triage existing-agent fix submission and reply receipts (2026-10-08).**
  Preview the exact edited instruction plus the TUI's correlated reply-draft
  return command, then explicitly send it to an existing running agent. Sending
  clears that terminal's input line and submits one bracketed paste. It starts no
  sessions, persists the fixing agent's provenance before delivery and marks the
  comment Fixing after successful submission. Changed PR heads, comments and
  replaced/stopped agent targets are refused. Cancellation retains edited text;
  ambiguous Enter errors retire the send confirmation and keep the fix editor,
  requiring a fresh explicit preview after inspecting the agent.
  The `amf reply-draft` CLI falls back to the existing shared SQLite request-id
  guard when the TUI socket is unavailable, so a standalone GUI receives drafts.
  **Reply: fixed** loads the fixing agent's returned draft and attribution through
  the TUI engine; editing and GitHub posting stay explicit.
  Validation: four Rust regressions and two frontend interactions; the full
  parallel workspace suite passes (3,406 library and seven GUI-crate tests,
  one pre-existing live-GitHub test ignored), along with all 293 frontend tests,
  production frontend/workspace builds, formatting and strict Clippy.
  Five asserted native WSLg frames cover exact-prompt confirmation, cancellation,
  stale-head refusal, real tmux delivery/Fixing and a returned attributed reply
  through the real CLI with no TUI socket. Private Git/SQLite/tmux and offline
  fixtures isolate the proof; no paid harness or GitHub write runs.
  Representative confirmation and reply PNGs were visually inspected. Reproduce with
  [`gui-pr-fix-submit.txt`](../../scripts/dev/screenshot/scenarios/gui-pr-fix-submit.txt).
  Dedicated/companion destinations, batching/integration, review memory and
  keep-as-TODO remain open; paid-agent delivery and macOS runtime remain unvalidated.

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
  - [x] **Review completion and explicit feedback handoff (2026-10-05).**
    Complete from the summary after a confirmation that lists the recorded
    counts, skipped files, saved apply-on-finish batch, configured check and PR
    posting. Completion shares the TUI finish order and recording engine
    (`record_final_review_round`, split from `complete_final_review`) and reruns
    the configured check instead of trusting a transient result. Handoff goes to
    the reviewed feature's first agent session only when chosen: submitted via
    the TUI's prompt delivery when it submits and the session runs, otherwise as
    an unsent composer draft. Stale revisions, patches, progress, check commands,
    agent sessions and deleted/moved features are refused before writing;
    cancelled or stale completion checks write nothing; completion is taken
    once. Seven Rust regressions and seven frontend interactions pass, with the
    full normal parallel workspace suite (3,263 library and seven GUI-crate
    tests; one existing live-GitHub test ignored), all 158 frontend tests, the
    production frontend build, formatting and strict workspace/all-target
    Clippy. Seven asserted native WSLg frames verify the flow through real Rust
    IPC with private fixtures; no agent receives anything. Reproduce with
    [`gui-review-complete.txt`](../../scripts/dev/screenshot/scenarios/gui-review-complete.txt).
    Destination choice, remaining native interactions and macOS validation stay
    open.
  - [x] **Finish-check execution (2026-10-05).**
    Preview and confirm the effective project check in the summary, inspect its
    bounded output and result, cancel or rerun it, and retain the open review.
    Share execution with the TUI, drain noisy commands without blocking and
    terminate/reap owned checks on close. Refuse obsolete launches/results when
    patches, saved progress, stable targets or configuration change. Completion
    and explicit feedback handoff remain the next increment. Nine new Rust
    regressions and seven frontend interactions pass. The full normal parallel
    workspace suite passes (3,253 library and seven GUI-crate tests; one existing
    live-GitHub test ignored), along with all 151 frontend tests, the production
    frontend build, both workspace executable builds, formatting and strict
    workspace/all-target Clippy. All 3,252 baseline Rust test names remain, with
    nine additions. Seven asserted native WSLg frames verify previews,
    confirmation, failed/passing output, cancellation and stale-result rejection
    through real Rust IPC. Invocation, source/progress and process assertions
    also verify draft retention, no feedback dispatch and descendant cleanup.
    Reproduce with [`gui-review-checks.txt`](../../scripts/dev/screenshot/scenarios/gui-review-checks.txt);
    local proof is `/tmp/amf-gui-review-checks-proof`. Remaining native
    interactions and macOS validation stay open.
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
    workspace/all-target Clippy. Completing the review and
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
  - [x] **Vibeless diff-review popup (2026-10-07).** Waiting edits open the
    existing review panel automatically over the active page or agent terminal.
    The configured answer hold restarts for each edit/revision. Other dialogs,
    menus and unsent input defer automatic opening while notices/badges remain.
    Escape/Close leaves requests waiting and restores prior focus; manually
    reopening remains available. Confirmed answers advance the oldest-first
    queue without remounting or moving focus. Manual reviews and reviews with
    automatic opening disabled remain open, including their empty state. Saved
    form values do not defer opening; the sidebar explains actual draft/dialog
    delays. Remaining counts use the fresh panel queue and other features,
    avoiding stale-count undercounts. A GUI-only preference (default on) can
    disable automatic opening.
    The loader, revision checks and answer delivery remain shared with the TUI.
    New popup mounts wait for a successful fresh hook-file read to avoid showing an older
    answered edit from cache. All 254 frontend tests (8 new regressions), the
    production build, full normal parallel Rust workspace suite (3,383 library
    and 7 GUI-crate tests; one pre-existing live-GitHub test ignored), formatting,
    strict workspace/all-target Clippy and both executable builds pass. The
    existing Rust hook test checks the configured hold in the GUI contract.
    Eight asserted native frames use real Rust IPC, private Git/SQLite/tmux,
    the shipped Claude hook and an offline agent stand-in. They verify automatic
    opening over the agent tab, dismissal without answering and restored focus,
    reopening, confirmed answers and refusal after an external answer. Expected
    text is checked after painting; representative images were inspected.
    Reproduce with [`gui-supervised-edits.txt`](../../scripts/dev/screenshot/scenarios/gui-supervised-edits.txt).
    Local proof: `/tmp/amf-gui-popup-proof`. No paid harness or GitHub write ran
    in the capture; macOS runtime validation remains open.

  - [x] **Supervised edits, first slice (2026-10-05).** Answer Vibeless edits that a Claude or
    OpenCode hook left on disk when no TUI is listening: navigation and feature
    badges, a one-time arrival notice, the hook's captured diff, and explicitly
    confirmed approve, reject-with-feedback and cancel answers that state each
    harness's effect. The reply format, file/IPC delivery and notification
    reader are shared with the TUI (`app/supervised_edits.rs`). Answers name a
    stable edit ID and revision; changed, already-answered and abandoned edits
    are refused and a departed hook's files are never recreated. Six new Rust
    regressions and six frontend interactions pass. The full normal workspace
    suite passes (3,262 library and seven GUI-crate tests; one existing
    live-GitHub test ignored), along with all 157 frontend tests, the
    production frontend build, both executable builds, formatting and strict
    workspace/all-target Clippy. Seven asserted native WSLg frames drive AMF's
    real Claude hook script with offline input and verify the arrival notice,
    side-by-side and new-file diffs, confirmations, approval (exit 0),
    rejection feedback delivery (exit 2) and refusal of an edit answered
    elsewhere. Reproduce with [`gui-supervised-edits.txt`](../../scripts/dev/screenshot/scenarios/gui-supervised-edits.txt).
    IPC-delivered requests stay in the TUI; AI edit explanations, OpenCode
    native frames and macOS validation remain open.
- [x] **Code syntax highlighting (2026-10-06).** Diffs, Final Review,
  supervised edits and the Learning reader colour code through the TUI's
  tree-sitter service (`src/gui_syntax.rs`): Rust sends token spans computed
  with whole-file context, so the interfaces agree on languages and parsers and
  the frontend bundles no highlighter. Plain-text fallback states its reason;
  missing or broken parsers install after confirmation. Colours are `--syn-*`
  CSS tokens with an AA contrast test. Reproduce with
  [`gui-syntax-highlighting.txt`](../../scripts/dev/screenshot/scenarios/gui-syntax-highlighting.txt).
- [x] **Prompt overrides manager (2026-10-06).** List every headless prompt
  with its effective layer, placeholders, effective and built-in templates.
  Save/clear feature, project (`amf.json`) and global overrides, shared or per
  harness, through the shared registry, resolver and stores. Per-prompt
  revisions refuse stale writes after external DB/`amf.json` edits. Unsaved
  edits are protected, and pending Final Review/planning pre-call notices link
  to the manager. Rust regressions, frontend interactions and ten asserted
  native WSLg frames
  ([`gui-prompt-overrides.txt`](../../scripts/dev/screenshot/scenarios/gui-prompt-overrides.txt))
  cover this increment.
- [ ] **Other GUI parity.** Continue settings and the remaining workflow
  inventory.
  - [x] **VS Code and custom sessions (2026-10-07).** The New session dialog
    offers the full TUI picker: VS Code through the TUI's tracked launch, the
    per-feature TODOs session, and `amf.json` custom sessions through the
    TUI's engine with `pre_check` failures shown in place and stale entries
    refused. A VS Code tab lists and closes tracked windows. Reproduce with
    [`gui-vscode-custom-sessions.txt`](../../scripts/dev/screenshot/scenarios/gui-vscode-custom-sessions.txt).
  - [x] **Dormancy (2026-10-06).** List idle, unattended features and stop a
    confirmed selection through the shared stop and editor-cleanup engine,
    with per-feature confirm-time re-checks and a full editor report. GUI
    session views now update `last_accessed` like TUI view entry. Reproduce with
    [`gui-dormancy.txt`](../../scripts/dev/screenshot/scenarios/gui-dormancy.txt).

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
