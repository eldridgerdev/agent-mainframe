# Learning Mode

- **Status:** In progress — Epic 1 (foundations) complete; Epic 2
  (browsing) next.
- **Owner:** unassigned
- **Relates to:** Final Review viewer (`src/app/review.rs`,
  `src/ui/dialogs/diff.rs`), Feature TODOs
  (`docs/backlog/feature-todos-plan.md`, `src/app/todos.rs`,
  `src/db/todos.rs`), headless runs (`src/headless.rs`), composer seed
  (`src/app/compose.rs` — `open_compose_seeded`), view modes
  (`src/app/state.rs` — `AppMode`).

## Why / problem

Add a Learning Mode overlay to AMF that lets a user browse a project's
files (whole tree or branch-changed), select a file / hunk / line
range, and ask an AI agent questions about that selection, with Q&A
history persisted per project.

**The user this is built for is new to the project — possibly new to
programming.** They have just cloned a repo they did not write, they do
not know where to start, they do not know the vocabulary the code uses,
and they are worried about breaking something. Every design choice
below is settled in that user's favour:

- **Nothing in Learning Mode mutates the repository.** The viewer is
  read-only; the only paths that produce a change (making an answer
  actionable, escalating to a live session) are explicit, confirmable,
  and pre-filled but never auto-submitted. This is stated in the UI,
  not just true in the code.
- **A blank prompt is never the only option.** Learning Mode ships with
  starter questions and an orientation ("start here") view so a user
  who does not yet know what to ask still has a first move.
- **Answers are written for a newcomer by default.** Prompts instruct
  the agent to assume no prior knowledge of this codebase, define
  jargon on first use, and end with a concrete "what to read next"
  pointer. A per-session level toggle (`Newcomer` / `Familiar`) exists
  for when the user outgrows that.
- **Follow-ups are first-class.** A newcomer's second question ("wait,
  what's a trait?") matters as much as their first, so Q&A rows thread:
  a follow-up carries its parent question and answer into the prompt.
- **Defaults are chosen so no configuration is required.** The harness
  defaults to the project's preferred agent; the picker exists but
  nobody has to open it.

Most questions are *explanatory* ("what does this line do?") and their
value is the answer itself, kept as a durable, anchored note. Some
questions are *directive* ("this should be its own function") and their
value is a change that gets made. Learning Mode supports both: every
answer is persisted as a note by default, and turning an answer into an
actionable follow-up (a TODO item, or a live agent session) is an
explicit, optional gesture available on any entry — never an assumed
outcome.

## Decisions

- **Surface:** a new `AppMode` overlay that reuses the Final Review
  file/diff viewer chrome (file list pane + content pane + selection
  affordances) rather than a tmux-backed session or a new dialog
  family. Final Review is `AppMode::DiffViewer(DiffViewerState)` with
  `review = true` (`src/app/review.rs:1951`); Learning Mode borrows its
  *chrome and interaction idiom*, not its state machine.
- **Entry point:** a dashboard key pressed on the selected
  feature/project, matching how PR Triage (`G`) and AI Review (`W`) are
  triggered from `handle_normal_key` (`src/handlers/normal.rs`). **No
  `SessionKind` row is created** — Learning Mode is not a session and
  does not appear in the session tree or switcher.
- **Browse scope:** both, with a toggle — (a) the full repo tree of the
  selected project/worktree, and (b) only files changed on the feature
  branch. The user may study current changes *or* study arbitrary code
  before spinning up an agent session in that worktree.
- **Orientation is part of repo-tree scope.** When repo-tree scope
  opens and the project has no prior Learning Mode history, the file
  list shows a pinned **Start here** group at the top, built by
  existence-checking a fixed candidate list in the workdir
  (`README.md`, `CLAUDE.md`, `AGENTS.md`, `CONTRIBUTING.md`,
  `src/main.rs`, `Cargo.toml` / `package.json` / `pyproject.toml` /
  `go.mod`), plus a repo-level "give me a tour of this project"
  question that anchors to the repo root rather than to a file. Missing
  candidates are simply absent; the group is collapsible and disappears
  once the user has asked anything in the project.
- **Selection granularity:** whole file, hunk, or line range are all
  valid question anchors. Repo-root ("whole project") is a fourth
  anchor used only by the orientation question.
- **Two question intents, one history.** Asking picks an intent:
  - **Explain** (default, labelled *"Explain this to me"* in the UI) —
    the answer is a teaching answer. No change is proposed, no
    follow-up is implied; the entry lives on as an anchored note.
  - **Action** (labelled *"Ask for a change"*) — the answer is a
    concrete change proposal (what to change, why, and a short title
    suitable for a work item).

  Intent only shapes the prompt framing and which follow-up action the
  UI offers first. It is stored per Q&A row and is **re-labelable after
  the fact**: an explanation that reveals a problem can be promoted to
  an action, and an action answer can be demoted to a note.
- **Explanation level is a per-session setting, not a per-question
  one.** `LearningLevel { Newcomer, Familiar }`, default `Newcomer`,
  stored on the learning session and shown in the header with a key to
  switch. It changes only prompt wording; it does not change tools,
  model, or which files are visible. Level is captured on each Q&A row
  so a reloaded answer explains why it reads the way it does.
- **Follow-up questions thread.** Any answered entry offers "ask a
  follow-up", which creates a new Q&A row with `parent_qa_id` set and
  the same anchor. The follow-up prompt includes the parent question
  and answer verbatim ahead of the new question, so "what's a trait?"
  resolves against what the user was just told. Threading depth is
  capped (see open questions) to bound prompt size.
- **Actionability is opt-in per answer, not a pipeline stage.** Any
  entry — whatever its intent — offers "make actionable", which creates
  a TODO-style item carrying the anchor plus an editable title/body
  seeded from the answer. Entries with no follow-up simply stay notes.
  v1 uses the queued-TODO path because AMF already has per-project TODO
  storage (`src/db/todos.rs`, `AmfDb::add_todo`) and a
  spawn-agent-from-item flow (`App::todos_spawn_agent`,
  `src/app/todos.rs`); the seeded-composer-scoped-to-range and
  inline-suggested-patch mechanisms are deferred, not rejected.
- **Execution model:** the first answer is produced **headless**
  (non-interactive, one-shot) via `HeadlessRunner`. The user can then
  **escalate the same Q&A into a live agent session**.
- **Headless context (v1):** start in `HeadlessRunner::run(...,
  restricted = true)` — the no-tools mode (`src/headless.rs:171`) —
  sending a context-complete prompt containing the selection plus
  surrounding file. A separate **"deep dive"** action reruns the same
  question through `HeadlessRunner::run_read_only`
  (`src/headless.rs:187`). For a newcomer both are described in the UI
  by what they do ("answer from just this file" vs "let the agent read
  the rest of the repo, slower"), not by their internal mode names.
- **Harness/model:** **user-selectable** in the Learning Mode UI —
  every harness AMF supports (Claude, Codex, Opencode, Pi) is
  selectable — but it is **pre-selected** to `store.available_harnesses`'
  first entry, falling back to the project's `preferred_agent`, exactly
  as the final-review harness pick does (`src/app/review.rs:3558`). A
  user who never opens the picker still gets a working run.
- **Persistence:** new SQLite tables plus a migration, scoped per
  project in the same style as `todo_lists` / `todos` (kept out of the
  `ProjectStore` JSON so store saves don't rewrite history).
- **Anchor drift:** v1 stores the raw path + line range and **accepts
  staleness**. A better solution (commit SHA + content snippet, or
  fuzzy re-resolution like `App::reanchor_line_comments`) is deferred
  to a tracked follow-up.
- **Async UX:** answering is **non-blocking**, and multiple questions
  may be **in flight at once** via a queue — the user can keep
  browsing, selecting, and asking while earlier answers generate.
  Because a newcomer will not know whether a stalled screen means
  "thinking" or "broken", every in-flight row shows an explicit status
  word and the header shows a count.

## Proposed design

### New state (`src/app/state.rs`)

- `AppMode::Learning(Box<LearningViewState>)` — a new variant alongside
  `Todos(TodoViewState)` and `DiffViewer(DiffViewerState)`.
- `LearningViewState`: `project_id`, `pi`/`fi`,
  `project_name`/`feature_name` (header labels, mirroring
  `TodoViewState`), `workdir`, `scope: BrowseScope { RepoTree,
  BranchChanges }`, file list (including the pinned orientation group)
  + cursor, loaded file content + viewport, current selection
  (`LearningAnchor::Project | File | Hunk { index } | Lines { start,
  end }`), focus pane, question editor (`crate::editor::TextEditor`),
  pending question intent, pending `parent_qa_id` for follow-ups, Q&A
  list + cursor, answer-pane scroll state (offset + rendered-line
  cache, as `draw_markdown_document` requires), selected harness,
  `level: LearningLevel`, and in-flight request tracking.
- `LearningQaIntent { Explain, Action }`, `LearningLevel { Newcomer,
  Familiar }`, `LearningRunMode { NoTools, DeepDive }`, and
  `LearningQaStatus { Pending, Running, Answered, Failed }`. Status
  display strings are full words ("queued", "thinking…", "answered",
  "failed"), not glyph-only.

### New app module (`src/app/learning.rs`)

Open/close the overlay, toggle browse scope, toggle level, load the
file list per scope (including orientation candidates), load file
content, move/extend the selection, submit a question *with an intent
and optional parent*, offer starter questions, relabel an entry's
intent, ask a follow-up, escalate to a live session, run a deep dive,
and create an actionable item from an answer. Mirrors the structure of
`src/app/todos.rs` and `src/app/review.rs`.

### Starter questions (`src/app/learning.rs`, constant table)

A static list of preset questions, each with an intent and the anchor
kinds it applies to. Opened with one key; picking one loads it into the
same `TextEditor` prompt so it can be edited before submitting.
Proposed v1 set, anchor-aware:

- project: "Give me a tour of this project — what is it, and where does
  execution start?"
- file: "What is this file responsible for, and what do I need to know
  to read it?" / "What calls into this file, and what does it call?"
- lines/hunk: "Explain this line by line." / "Why is it written this
  way instead of the obvious way?" / "What would break if I deleted
  this?" / "What do the unfamiliar words here mean?"
- action intent: "Suggest how to make this clearer without changing
  behaviour."

Rendered with the existing list-picker shape (`src/ui/picker.rs`, and
the `ReviewHarnessPickState` picker at
`src/ui/dialogs/review_harness.rs`).

### File sources

- *Branch changes*: `crate::diff::load_snapshot(&workdir,
  override_base_ref, ignore_whitespace)` (`src/diff.rs:524`) — the same
  call the diff viewer makes in `App::complete_diff_viewer_loading`. It
  returns `DiffFile`s carrying `hunks`, `addressable_lines()`,
  `hunk_start_indices()` and `addressable_line_texts()`, so hunk
  selection and selection-text capture come for free on this side.
- *Repo tree*: no `ignore`/`walkdir` crate is in `Cargo.toml`, so
  enumerate with `git ls-files --cached --others --exclude-standard`.
  Add this as a `pub fn list_repo_files(workdir) -> Result<Vec<String>>`
  in `src/diff.rs` beside the existing `list_untracked_files`, which
  already runs `ls-files --others --exclude-standard` through the
  module-private `git_capture` — reusing that helper rather than
  opening a second `Command::new("git")` call site. This inherits
  `.gitignore` handling from git rather than reimplementing it. For
  non-git projects (`Project.is_git == false`) fall back to a plain
  recursive walk with a depth/entry cap. Skip binary and oversized
  files at load time. In this scope there are no hunks, so hunk
  selection is unavailable and the UI offers project/file/line
  selection only.
- File content rendering reuses `crate::highlight` the way the diff
  viewer does (`highlight::language_install_state_for_path`).

### Agent execution (`src/headless.rs` callers in `src/app/learning.rs`)

Prompt builders are pure functions selected by intent, with the level
and any parent Q&A applied on top:

- *Explain*: repo/feature identity, file path, the selected text with
  line numbers, surrounding file context, and an instruction to explain
  what the code does and why it is written that way — explicitly **not**
  to propose changes unless the user asked.
- *Action*: the same context, plus an instruction to propose the
  smallest concrete change that satisfies the user's request and to
  lead with a single-line imperative summary (used verbatim as the
  seeded TODO title when the user chooses to make it actionable)
  followed by rationale.
- *Newcomer level* (default) adds: assume the reader has never seen
  this codebase and may be new to the language; define every technical
  term the first time it appears; prefer short paragraphs and concrete
  examples over abstraction; do not assume familiarity with the
  project's own vocabulary; finish with a short "**Where to look
  next**" list of specific files or symbols and why each is worth
  reading.
- *Familiar level* drops the define-your-terms and where-to-look-next
  requirements and asks for a denser answer.
- *Follow-up*: the parent question and parent answer are inserted
  before the new question under an explicit "earlier in this
  conversation" heading, so the agent answers in context without
  re-deriving it.

Question → `HeadlessRunner::run(harness, workdir, prompt, model,
/* restricted */ true)`. Deep dive → `HeadlessRunner::run_read_only(
harness, workdir, prompt, model)` in the feature's `workdir`, same
question, intent, and level, tools allowed to explore.

Both calls block, so each runs on its own thread. Follow AMF's
established async shape: `std::sync::mpsc::channel()` +
`std::thread::spawn` (as in `src/app/ai_review.rs`) with a
`poll_learning_answers_bg()` drained from the main loop next to the
existing `poll_*_bg` calls (`src/main.rs`). Multiple pending runs are
allowed; each Q&A row carries its own status.

### Escalation to a live session

Creates an agent-harness session on the selected feature via
`create_agent_session_labeled` (as `todos_spawn_agent` does), enters it
with `enter_view_without_auto_compose` (`src/app/view.rs:45`), and
seeds the composer with `open_compose_seeded`
(`src/app/compose.rs:411`) — editable, not auto-submitted, exactly as
`todos_spawn_agent` does. The seed carries the anchor, the question,
and the headless answer, and is phrased by intent (an Explain entry
escalates as "here is what I was asking about, continue explaining"; an
Action entry escalates as "please make this change"). At `Newcomer`
level the seed also asks the live agent to explain what it is doing as
it goes. Store the resulting session id on the Q&A row so a second
escalation jumps back to the linked session instead of spawning
another.

### Making an answer actionable

Loads/creates the project's list via
`load_or_create_todo_list(project_id, feature_id)`
(`src/db/todos.rs:123`; note the one-list-per-project constraint) and
opens a small editor pre-filled with a title (the Action answer's lead
line, or a truncated first line for an Explain answer) and a body
containing `path:start-end`, the question, and the answer excerpt.
Written via `AmfDb::add_todo`. Nothing is written until the user
confirms, since an auto-generated title from an explanatory answer is
usually wrong. The confirm dialog states in plain language that this
only adds a note to the project's TODO list and changes no files. The
created `todos.id` is stored on the Q&A row so the entry renders as
"actioned →" and repeat invocation jumps to the item instead of
duplicating it.

### Persistence (`src/db/learning.rs` + `MIGRATION_017`)

- `learning_sessions` — `id`, `project_id`, `feature_id`, `title`,
  `harness`, `level` (`newcomer` / `familiar`), `onboarding_seen`
  (INTEGER, drives the first-open help overlay), `created_at`,
  `updated_at`.
- `learning_qa` — `id`, `learning_session_id`, `parent_qa_id`
  (nullable, self-referencing for follow-up threads), `file_path`
  (nullable for the project-level anchor), `anchor_kind`, `line_start`,
  `line_end`, `selection_text`, `question`, `intent` (`explain` /
  `action`), `level`, `answer`, `harness`, `run_mode` (`no_tools` /
  `deep_dive`), `status`, `todo_id` (nullable — set only when the user
  made it actionable), `spawned_session_id`, `created_at`,
  `updated_at`.
- Append `("Add learning_sessions + learning_qa tables for Learning
  Mode", MIGRATION_017)` to the migration list in
  `src/db/migrations.rs` (current tail is `MIGRATION_016`, schema
  version 16; the loop derives the target version from array position,
  so appending is sufficient) and follow `MIGRATION_011`'s todo-table
  shape: plain TEXT `project_id`/`feature_id` with no FK to
  projects/features, explicit delete helpers, `ON DELETE CASCADE` from
  session to Q&A rows and from a parent Q&A row to its follow-ups.
- `AmfDb` methods to load/upsert a learning session and
  list/upsert/delete Q&A rows, plus
  `delete_learning_sessions_for_project`, wired into project deletion
  the way `delete_list_for_project` is.
- As with todos, the in-memory list is the overlay's source of truth
  and works without a DB; the DB persists it when present.

### Key handling (`src/handlers/learning.rs`)

New module registered in the main dispatch, following
`src/handlers/todos.rs`. `handle_normal_key` gains the entry key.

### Rendering (`src/ui/dialogs/learning.rs`)

Matches the existing overlay convention (`src/ui/dialogs/todos.rs`,
`src/ui/dialogs/diff.rs`): file list, syntax-highlighted or
diff-rendered content, selection highlight, Q&A panel. **Answers render
as markdown** through `super::markdown::draw_markdown_document`
(already reused this way by the plan-interview review gate), so
headings, bullet lists, and fenced code in an answer read as formatted
text rather than raw markup. This carries the scroll-offset /
rendered-width / rendered-lines state the function requires on
`LearningViewState`.

## UI

- **Entry:** a dashboard key on the selected feature/project. `L` is
  **not** available — it opens the prompt library. A full sweep of
  `handle_normal_key` confirms the taken set is `q N B O A n c x d s S
  u y h l ? / i r R V j k F T p L G W P f Z D` plus `Enter`/arrows.
  **`K`** ("knowledge") is free and is the proposal; register it in
  `DASHBOARD_KEYBINDING_ACTIONS` (`src/handlers/normal.rs`) as
  `("learning_mode", 'K')` so it is user-rebindable and appears in the
  config wizard, and add it to the `?` help overlay list
  (`src/ui/dialogs/help.rs`) — a newcomer's discovery path is that
  overlay, not the source.
- **Layout:** three regions in Final Review's visual idiom — file list
  (left, with the pinned **Start here** group on top in repo-tree
  scope), file/diff content with line numbers (center), Q&A history for
  the current file (right or bottom).
- **Read-only reassurance:** the header carries a persistent
  `read-only` marker, and the help overlay opens the first time a
  project enters Learning Mode (`onboarding_seen`), leading with what
  the mode does, that it changes no files, and which two keys ask a
  question.
- **Scope toggle:** a visible indicator showing `Repo tree` vs `Branch
  changes` with a key to switch; the file list reloads in place and the
  header states which scope is active, spelled out ("all files in this
  project" / "files changed on this branch").
- **Selection:** cursor line is the default anchor; a key starts a
  multi-line range (Final Review's multi-line comment interaction), and
  in branch-changes scope a key selects the enclosing hunk. Whole-file
  selection is its own key. The active anchor is rendered as a
  highlighted gutter/region and echoed in plain words above the
  question input.
- **Ask — two keys, one for each intent:** *"Explain this to me"* and
  *"Ask for a change"* both open the same inline `TextEditor` prompt,
  showing the resolved anchor and the chosen intent in the prompt's
  title bar. A key inside the prompt flips intent before submitting;
  another key opens the **starter questions** picker to fill the prompt
  instead of typing from scratch. Submit enqueues the run and
  immediately returns control — the overlay stays fully interactive.
- **Level indicator:** the header shows `Explaining for: newcomer` (or
  `familiar`) with the key to switch, so the user knows why answers
  read the way they do and can change it without hunting.
- **Q&A entries** render with an intent marker and word (`? explain` /
  `! change`), a status word (queued / thinking… / answered / failed),
  harness, and run mode described plainly ("this file only" / "read the
  repo"). Follow-ups render indented under their parent. Actioned
  entries additionally show a `→ TODO` marker; entries linked to a live
  session show `→ session`. A header counter shows how many answers are
  still generating. Failures show the error text and offer retry.
- **Answer view:** selecting an entry shows the full answer as rendered
  markdown in a scrollable pane. Actions, in the order the intent makes
  most likely: **ask a follow-up**, **deep dive** (rerun letting the
  agent read the repo), **escalate to live session**, **make
  actionable** (opens the confirm/edit dialog described above),
  **relabel intent**, **delete**. All actions are available on every
  entry regardless of intent — only their ordering/emphasis differs.
  The footer lists every available key with a spelled-out label rather
  than a glyph legend.
- **Harness picker:** a key opens a picker listing available harnesses;
  the choice is shown in the header and applies to subsequent runs.
  Reuse the `ReviewHarnessPickState` list-picker shape. Pre-selected
  from `store.available_harnesses` / `preferred_agent`, so it is
  optional.
- **Help:** `?` opens a Learning Mode key overlay, matching the Final
  Review help overlay convention (`App::open_review_help`). It states
  plainly that Learning Mode never edits files, that explaining and
  acting are separate and both optional, that answers are generated by
  the selected agent CLI and therefore cost whatever that agent costs,
  and that no question is too basic.

## Progress

Epics are ordered by dependency, not by importance. **Epic 1 gates
everything**; after it, Epic 2 (browsing) and Epic 3 (prompts +
execution) are independent of each other and can be built in parallel,
as can Epic 4's rendering and key handling once Epic 2's state is in
place. Epic 5 depends on Epic 3 having produced real answers. Epic 6
closes out.

### Epic 1 — Foundations (state, persistence, file sources)

- [x] Add `LearningViewState`, `BrowseScope`, `LearningAnchor`
      (including the project-level anchor), `LearningQaIntent`,
      `LearningLevel`, `LearningRunMode`, and `LearningQaStatus` to
      `src/app/state.rs`; add `AppMode::Learning(Box<LearningViewState>)`
      to the `AppMode` enum. Verified with `cargo check`.
- [x] Add `MIGRATION_017` (`learning_sessions` with
      `level`/`harness`/`onboarding_seen`; `learning_qa` including
      `intent`, `level`, nullable `parent_qa_id`, `todo_id`,
      `spawned_session_id`) to the list in `src/db/migrations.rs`
      following the `MIGRATION_011` todo-table pattern; add
      `src/db/learning.rs` with load/create/list/upsert/delete methods
      on `AmfDb`, plus per-project cleanup mirroring
      `delete_list_for_project` (wired into `delete_project`).
      Verified: `migration_017_upgrades_a_v016_database`,
      `fresh_database_lands_at_the_latest_version`,
      `migrations_are_idempotent`, twelve `db::learning` round-trip /
      cascade tests, and a manual replay against a copy of the real
      `~/.config/amf/amf.db` (v16 → 17, 8 projects, cascades both ways,
      `integrity_check` ok). Two notes: `learning_qa` also carries an
      `error` column (a failed row must reload with its reason), and
      `project_id` is deliberately not UNIQUE — one-session-per-project
      is enforced in `load_or_create_session` while the lifecycle
      question stays open.
- [x] Add `pub fn list_repo_files(workdir) -> Result<Vec<String>>` to
      `src/diff.rs` beside `list_untracked_files`, running `git
      ls-files --cached --others --exclude-standard` through the
      existing `git_capture` helper. Output is sorted, deduped, and
      filtered to paths that are actually on disk (a tracked-but-deleted
      file can't be browsed). Tests:
      `list_repo_files_covers_tracked_and_unignored_untracked_files`,
      `list_repo_files_skips_tracked_files_deleted_from_disk`.

### Epic 2 — Browsing (overlay, file list, content, selection)

- [ ] Implement `src/app/learning.rs` open/close plus file-list loading
      for both scopes: `crate::diff::load_snapshot` for
      `BranchChanges`, `list_repo_files` (with a capped plain walk for
      non-git projects) for `RepoTree`. Verify by opening the overlay
      on a feature and toggling scopes, including on a non-git project.
- [ ] Build the **Start here** orientation group: existence-check the
      fixed candidate list in the workdir, pin the surviving entries
      above the repo-tree file list, hide the group once the project
      has any Q&A history, and expose the repo-level "tour this
      project" question. Unit-test candidate resolution against a temp
      dir with a partial candidate set and against one with none.
- [ ] Implement file content loading (binary/size skip) and the
      selection model — project / file / hunk / line range — with hunk
      selection unavailable in `RepoTree` scope. Reuse
      `DiffFile::hunk_start_indices()` and `addressable_line_texts()`
      for hunk anchors and selection text. Add unit tests for anchor
      construction, range clamping, and hunk-anchor absence in
      repo-tree scope.

### Epic 3 — Asking (prompts, headless execution, async queue)

- [ ] Implement the prompt builders as pure functions with unit tests:
      explain (teach, do not propose changes), action (propose the
      smallest concrete change, lead with a one-line imperative
      summary), the `Newcomer` overlay (define terms, no assumed
      familiarity, trailing "Where to look next"), the `Familiar`
      overlay, and the follow-up wrapper (parent question + parent
      answer precede the new question). Assert each includes
      repo/feature identity, path, numbered selection, and surrounding
      context; that the explain template contains no change-request
      language; that the newcomer overlay is absent at `Familiar`
      level; and that a follow-up prompt contains its parent's text
      exactly once.
- [ ] Implement headless question submission through
      `HeadlessRunner::run(..., restricted = true)` using the intent-
      and level-selected prompt. Verify against a real file with each
      available harness, checking that a newcomer-level explain answer
      defines its terms and ends with a "Where to look next" section,
      and that an action answer opens with a usable one-line title.
- [ ] Implement the non-blocking async queue: `mpsc` +
      `thread::spawn` per run, a `poll_learning_answers_bg()` drained
      in `src/main.rs` alongside the existing `poll_*_bg` calls,
      per-row status transitions (`Pending → Running →
      Answered|Failed`) rendered live with word labels, and results
      persisted to the DB on completion. Verify by enqueuing three
      questions across two files with mixed intents and confirming the
      overlay stays interactive and all three resolve independently.
- [ ] Implement the harness picker over `store.available_harnesses`
      with the `preferred_agent` fallback, persisted on the learning
      session and used by the initial run, follow-ups, and deep dive.
      Verify the overlay works end to end without ever opening the
      picker.
- [ ] Implement **level toggle** (`Newcomer` ⇄ `Familiar`), persisted
      on the learning session, applied to subsequent runs only, with
      each Q&A row recording the level it was answered at. Verify the
      header updates and an existing answer is not rewritten.

### Epic 4 — Surface (rendering, keys, entry point, onboarding)

- [ ] Build `src/ui/dialogs/learning.rs`: file list with the
      orientation group, content with line numbers and
      `crate::highlight` syntax, selection highlight, Q&A panel with
      intent + status words + threaded follow-ups + actioned/session
      markers, answers rendered through
      `markdown::draw_markdown_document`, and a header showing scope,
      level, harness, `read-only`, and in-flight count. Verify a
      markdown answer with headings, a list, and a fenced code block
      renders formatted and scrolls to the end.
- [ ] Add `src/handlers/learning.rs` and wire it into the main key
      dispatch: navigation, scope toggle, level toggle, selection keys,
      the two ask keys (explain / change), starter-question picker,
      intent flip inside the prompt, follow-up, answer-pane scrolling,
      harness picker, and `?` help overlay. Add handler tests in the
      style of `src/handlers/diff.rs` covering help-overlay open/close
      and key swallowing while it is open.
- [ ] Implement the first-open behaviour: show the help overlay when
      the project's learning session has `onboarding_seen = 0`, then
      set and persist the flag. Verify it appears once and not on the
      second open.
- [ ] Add the dashboard entry key `K` (verified unbound in
      `handle_normal_key`) on the selected feature/project, register it
      in `DASHBOARD_KEYBINDING_ACTIONS`, and add it to the dashboard
      help overlay list. Verify every existing dashboard binding still
      works, especially `L` (prompt library), and that `K` appears in
      the config wizard's bindable actions.
- [ ] Implement the starter-question table and picker (anchor-aware
      filtering, loads into the editable prompt). Unit-test that
      project-anchor presets are offered only for the project anchor
      and line presets only when a line/hunk range is selected.

### Epic 5 — Acting on an answer

- [ ] Implement **ask a follow-up**: new Q&A row with `parent_qa_id`,
      same anchor, parent Q&A embedded in the prompt, rendered indented
      under its parent, with a depth cap that trims the oldest
      ancestors from the prompt beyond the cap. Verify a two-deep
      follow-up answers in context and that the cap keeps prompt size
      bounded.
- [ ] Implement **deep dive**: rerun the selected Q&A through
      `HeadlessRunner::run_read_only` in the feature's `workdir`,
      preserving intent and level, stored as its own row with
      `run_mode = deep_dive` so the original answer survives.
- [ ] Implement **relabel intent** on an existing entry (explain ⇄
      change), persisted, with the answer text left untouched. Verify a
      relabeled entry re-renders with the new marker and re-orders its
      action affordances.
- [ ] Implement **make actionable** as an explicit, confirmable action
      on any entry: `load_or_create_todo_list(project_id, feature_id)`,
      pre-fill an editable title (action lead line, or truncated first
      answer line for an explain entry) and a body containing
      `path:start-end` + question + answer excerpt, write via
      `AmfDb::add_todo`, and store `todo_id` on the Q&A row. The
      confirm dialog states plainly that this writes a note, not code.
      Verify (a) nothing is written on cancel, (b) the item appears in
      the project's TODO list and can spawn an agent through the
      existing flow, and (c) re-invoking on an already-actioned entry
      jumps to the existing item instead of creating a duplicate.
- [ ] Implement **escalate to live session**: create an agent-harness
      session on the feature via `create_agent_session_labeled`,
      `enter_view_without_auto_compose`, and `open_compose_seeded` with
      an intent- and level-appropriate seed (anchor + question +
      answer, editable, not auto-submitted); store
      `spawned_session_id` and jump back to the linked session on
      repeat escalation. Verify the composer is pre-filled and unsent
      for both intents, and that the newcomer-level seed asks the live
      agent to narrate what it is doing.

### Epic 6 — Hardening and docs

- [ ] Add error handling and debug logging (`log_info` / `log_warn` /
      `log_error` with a `"learning"` context) for file load failures,
      headless run failures, and DB errors. User-facing errors must say
      what to do next, not just what failed (e.g. a missing harness CLI
      points at the `A` harness wizard). Confirm no
      `println!`/`eprintln!` was introduced.
- [ ] Add tests covering: DB round-trip of a session plus Q&A rows
      including `intent`, `level`, `parent_qa_id`, `todo_id`, and
      `spawned_session_id`; follow-up cascade on parent delete; the
      no-DB in-memory path; scope toggling; anchor serialization
      (including the project anchor); and that an answered explain
      entry with no follow-up persists and reloads unchanged (the
      default, non-actioned path).
- [ ] Run `cargo build`, `cargo clippy`, and the test suite; fix all
      warnings introduced by this feature.
- [ ] Update `README.md` (feature bullet, the dashboard keybindings
      table, and a `### Learning Mode` section written for a first-time
      reader: what it is for, that it never edits files, the two ask
      keys, starter questions, and the newcomer/familiar levels),
      `CLAUDE.md` (new `src/app/learning.rs`,
      `src/handlers/learning.rs`, `src/ui/dialogs/learning.rs`,
      `src/db/learning.rs` under the `## Architecture` sections, plus a
      Learning Mode section describing the explain/change split and the
      level/threading model), and `CHANGELOG.md`.
- [ ] File follow-up items for the deferred work: anchor-drift
      resolution (commit SHA + snippet or fuzzy match, modeled on
      `App::reanchor_line_comments`), the alternative actionable
      mechanisms (composer seeded and scoped to file/range, inline
      suggested patch like Final Review suggestions), and turning
      "Where to look next" file references into navigable jumps within
      the overlay.

## Open questions

- **Reading level is a prompt instruction, not a guarantee.** The
  `Newcomer` overlay asks the agent to define its terms and avoid
  assumed context, but nothing enforces it; a model may still answer in
  jargon, and different harnesses will comply unevenly. There is no
  automated check for "is this understandable", so verification is
  manual. If compliance turns out poor, the fallback is a stricter
  template or a post-answer "explain that more simply" one-key rerun —
  not planned for v1.
- **The starter-question list is a guess.** It is written for a
  Rust/TUI codebase reader and has had no user testing. Whether these
  are the questions a beginner actually has — and whether the list
  should be per-language or user-editable (the prompt library already
  stores user templates) — is open.
- **The "Start here" candidate list is a heuristic.**
  Existence-checking a fixed set of well-known filenames will do
  nothing useful for a project that follows none of those conventions,
  and it does not rank by usefulness. It degrades to an empty group,
  which is safe but unhelpful.
- **Follow-up threading grows prompts.** Each level adds a full
  question + answer. The depth cap bounds this, but the right cap is
  unset, and trimming ancestors can silently drop the context a later
  question depends on.
- **The final actionable mechanism is undecided.** v1 uses the queued
  TODO because it reuses the most existing machinery. What is settled
  is the *shape*: actionability is an optional, explicit action on an
  answer, not the terminal step of every Q&A. The specific mechanism
  behind that action remains open.
- **Intent is user-declared, not inferred.** v1 asks the user to choose
  explain vs change at ask time (with relabeling afterwards). A
  newcomer is exactly the user most likely to pick the wrong one —
  typing "this should be its own function" under the explain key gets
  an explanation. Relabel + follow-up mitigate this; automatic
  classification is untested and not planned for v1.
- **Seeded TODO titles from explanatory answers are likely poor.** The
  action template is designed to produce a usable one-line title; an
  explain answer has no such line, so the pre-filled title is a
  truncation the user must edit. Mitigated by requiring confirmation,
  but it may still be friction.
- **TODO-list noise.** Learning Mode writes into the same one-per-project
  list as the TODOs overlay. Whether learning-originated items need
  visual distinction or a separate list is undecided.
- **Anchor staleness is a known, accepted v1 defect.** Q&A entries
  reference `path:line-range` with no drift protection, so cleaning up
  a file will silently misalign earlier entries — and explanatory notes
  are exactly the entries meant to be long-lived, so this bites the
  primary use case first. It is also the failure a newcomer is least
  equipped to recognise: a stale anchor looks like a wrong answer.
- **Entry key `K` is proposed but not user-validated.** Verified
  unbound in `handle_normal_key`, and `L` is confirmed taken by the
  prompt library, but the mnemonic is a judgment call. Being in
  `DASHBOARD_KEYBINDING_ACTIONS` makes it rebindable if it reads badly.
- **Learning-session lifecycle is unspecified.** Unknown: one learning
  session per project or many; how a session is named/created; and what
  happens to history when the associated feature or worktree is
  deleted. Todos solve the analogous problem with a host-feature
  reassign prompt (`AppMode::TodosHostReassign`) — whether Learning
  Mode needs the same is undecided.
- **Repo-tree scope on large repositories.** `git ls-files` output and
  content loading cost are unbounded; the specific entry-count,
  file-size, and binary-detection limits are unset. The non-git
  fallback walk is a further unknown, since it has no ignore rules at
  all — and an unfamiliar user browsing a monorepo is the worst case
  for both.
- **Hunk selection only exists in branch-changes scope.** Repo-tree
  browsing has no diff, so "hunk" has no meaning there. This asymmetry
  is an inference from the two scopes, not a stated decision, and the
  missing key may read as a bug to someone new.
- **Queue concurrency limits are unset.** "Multiple in flight" is
  decided; the maximum number of concurrent headless runs, and whether
  runs are capped per harness (rate limits, cost), are not.
- **Cost/usage attribution is unaddressed** — and this matters more for
  the target user, who may not realise each question spends money. AMF
  tracks token usage per agent session, but Learning Mode's headless
  runs have no session row. `HeadlessRunner::run` / `run_read_only` do
  not report usage — only `run_with_progress` does — so surfacing cost
  would mean either switching run paths (losing the restricted no-tools
  boundary, which `run_with_progress` does not offer) or leaving these
  runs unattributed. v1 leaves them unattributed and only warns in the
  help overlay.
- **Non-git and non-worktree projects.** Branch-changes scope is
  meaningless when `Project.is_git` is false; the plan degrades to
  repo-tree-only there, but this was not explicitly requested.
- **No stated requirement for editing files from within Learning
  Mode.** The viewer is read-only; all mutation happens through
  escalation or the actionable item. This is a deliberate property of
  the newcomer framing, so relaxing it later should be a conscious
  decision rather than a convenience patch.

## Reasoning / when to build

Being built now, on the `learning-mode` branch. The feature exists to
make an unfamiliar codebase approachable without asking the user to
learn AMF's agent workflow first — so the newcomer framing above (no
mutation, starter questions, defined jargon, cheap follow-ups) is the
product, not decoration around it. Anything that trades that framing
for developer convenience should be treated as a change of scope, not a
tweak.
