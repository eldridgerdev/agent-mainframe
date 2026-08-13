# Learning Mode

- **Status:** In progress — Epics 1 (foundations), 2 (browsing), 3
  (asking), 4 (surface), and 5 (acting on an answer) are complete, and Epic
  6 is done apart from filing the deferred follow-ups. Learning Mode is
  reachable from the dashboard with `K`, usable end to end (browse, select,
  ask, read the answer, ask a follow-up, send a doubtful answer back with
  the repo readable, re-file an entry as the other kind of question, keep an
  answer as a to-do, and hand one to a live agent session), and documented
  in `README.md`, `CLAUDE.md`, and `CHANGELOG.md`. What remains is filing
  the deferred follow-ups as their own backlog items, plus Epic 7 (a
  collapsible file tree, so the file list teaches the repo's layout instead
  of hiding it behind truncated paths), which blocks on nothing and can run
  in parallel.
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

### Persistence (`src/db/learning.rs` + `MIGRATION_019`)

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
  `updated_at`, and `selection_is_diff` (added by `MIGRATION_020`).
  `selection_is_diff` cannot be re-derived from the other columns: a
  line anchor looks the same whether it came from the repo tree or from
  a diff, and the browse scope that told them apart is not stored — so a
  follow-up would otherwise label its parent's excerpt from wherever
  browsing had since ended up. Rows written before it default to 0.
- Append `("Add learning_sessions + learning_qa tables for Learning
  Mode", MIGRATION_019)` to the migration list in
  `src/db/migrations.rs` (the tail when this was written was
  `MIGRATION_016`; the editor-tracking pair landed first, so the
  learning tables became 019. The loop derives the target version from
  array position, so appending is sufficient) and follow
  `MIGRATION_011`'s todo-table
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
closes out. **Epic 7 blocks on nothing** — it reworks the file list
Epic 2 built, touches no prompt, run, or persistence code, and can be
picked up in parallel with whatever is left of Epics 5 and 6.

### Epic 1 — Foundations (state, persistence, file sources)

- [x] Add `LearningViewState`, `BrowseScope`, `LearningAnchor`
      (including the project-level anchor), `LearningQaIntent`,
      `LearningLevel`, `LearningRunMode`, and `LearningQaStatus` to
      `src/app/state.rs`; add `AppMode::Learning(Box<LearningViewState>)`
      to the `AppMode` enum. Verified with `cargo check`.
- [x] Add `MIGRATION_019` (`learning_sessions` with
      `level`/`harness`/`onboarding_seen`; `learning_qa` including
      `intent`, `level`, nullable `parent_qa_id`, `todo_id`,
      `spawned_session_id`) to the list in `src/db/migrations.rs`
      following the `MIGRATION_011` todo-table pattern; add
      `src/db/learning.rs` with load/create/list/upsert/delete methods
      on `AmfDb`, plus per-project cleanup mirroring
      `delete_list_for_project` (wired into `delete_project`).
      Verified: `migration_019_upgrades_a_pre_learning_database`,
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

- [x] Implement `src/app/learning.rs` open/close plus file-list loading
      for both scopes: `crate::diff::load_snapshot` for
      `BranchChanges`, `list_repo_files` (with a capped plain walk for
      non-git projects) for `RepoTree`. Verified by overlay-level tests
      that open on a real temp repo and toggle scopes
      (`opening_lists_repo_files_with_the_orientation_group_on_top`,
      `toggling_scope_switches_to_the_branch_s_changed_files_and_back`,
      `a_non_git_project_stays_in_repo_tree_scope_and_explains_why`,
      `the_overlay_works_without_a_database`,
      `closing_returns_to_the_feature_it_was_opened_from`). Limits now
      set: 2 MB per file, 20k repo entries, walk depth 12, and a short
      skip list for the non-git walk (whether those are the right
      numbers stays open below). Repo-tree is the default scope, and
      branch-changes is refused with an explanation on a non-git
      project.
- [x] Build the **Start here** orientation group: existence-check the
      fixed candidate list in the workdir, pin the surviving entries
      above the repo-tree file list, hide the group once the project
      has any Q&A history, and expose the repo-level "tour this
      project" question. Tests: `start_here_lists_only_files_that_exist`,
      `start_here_is_empty_for_a_project_following_no_conventions`,
      `start_here_ignores_directories`,
      `repo_tree_entries_pin_the_orientation_group_on_top`,
      `collapsing_the_group_keeps_only_its_header`,
      `collapsing_the_orientation_group_keeps_the_cursor_on_its_file`.
      The cursor opens on the tour question rather than the group
      header, so the first thing a newcomer sees is a question they can
      ask.
- [x] Implement file content loading (binary/size skip) and the
      selection model — project / file / hunk / line range — with hunk
      selection unavailable in `RepoTree` scope. Tests:
      `binary_and_oversized_files_are_skipped_with_a_reason`,
      `text_files_load_as_lines`, `line_anchor_is_one_based_and_clamped`,
      `empty_file_anchors_to_the_file`,
      `selection_text_covers_the_anchored_lines_only`,
      `hunk_lookup_finds_the_enclosing_hunk`,
      `hunk_selection_is_unavailable_in_repo_tree_scope`,
      `hunk_selection_works_only_once_a_changed_file_is_loaded`,
      `selecting_a_file_loads_its_content_and_a_line_anchor`. Pressing
      the hunk key in repo-tree scope says why there's no hunk instead
      of doing nothing, which closes the "missing key may read as a
      bug" concern below.

### Epic 3 — Asking (prompts, headless execution, async queue)

- [x] Implement the prompt builders as pure functions with unit tests:
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
      exactly once. Done as `build_prompt` + `intent_instructions` +
      `level_instructions`, with nine tests including ancestor trimming
      at `MAX_FOLLOW_UP_DEPTH` and selection truncation.
- [x] Implement headless question submission through
      `HeadlessRunner::run(..., restricted = true)` using the intent-
      and level-selected prompt. Verified end to end against a real
      file (`src/app/learning.rs:300-312`) through the real Claude
      no-tools path: the answer defined its Rust terms before using
      them, walked the selection line by line, proposed no changes, and
      closed with a "Where to look next" list. **Only Claude has been
      run for real so far** — Codex and Opencode are installed and
      unverified; Pi is not installed. Unit tests spawn no CLI (see the
      `cfg!(test)` guard in `spawn_learning_run`), so the suite costs
      nothing to run.
- [x] Implement the non-blocking async queue: a persistent `mpsc`
      channel on `App` (not a one-shot slot, which is what allows
      several runs at once) plus a thread per run, with
      `poll_learning_answers_bg()` drained in `src/main.rs` beside the
      other `poll_*_bg` calls, per-row status transitions, and results
      persisted on completion. Tests cover enqueue-and-return,
      out-of-order completion, in-flight counting, and failure rows
      that keep the question for a retry.
- [x] Implement the harness picker over `store.available_harnesses`
      with the `preferred_agent` fallback, persisted on the learning
      session and used by later runs. It lives inside
      `LearningViewState` rather than as its own `AppMode`, so opening
      it can't lose the browsing state behind it. Tests confirm it is
      pre-selected on the harness in use, that cancelling changes
      nothing, and that questions record the harness that answered
      them — the picker never has to be opened.
- [x] Implement **level toggle** (`Newcomer` ⇄ `Familiar`), persisted
      on the learning session, applied to subsequent runs only, with
      each Q&A row recording the level it was answered at
      (`toggling_level_affects_later_questions_only` asserts the
      earlier row keeps both its level and its answer text). The header
      itself lands with the renderer in Epic 4.

### Epic 4 — Surface (rendering, keys, entry point, onboarding)

- [x] Build `src/ui/dialogs/learning.rs`: file list with the
      orientation group, content with line numbers and
      `crate::highlight` syntax, selection highlight, Q&A panel with
      intent + status words + threaded follow-ups + actioned/session
      markers, answers rendered through
      `markdown::draw_markdown_document`, and a header showing scope,
      level, harness, `read-only`, and in-flight count. Modal layers
      draw in the same precedence the key handler uses. Ten tests,
      seven of them rendering through a `TestBackend`:
      `a_markdown_answer_renders_formatted` (the plan's check — heading
      markers and code fences are consumed, the code block's contents
      and list items survive), `a_long_answer_scrolls_to_its_end`
      (scroll-to-bottom lands on the last line and the stored scroll is
      clamped), `the_header_says_what_the_mode_is_doing_and_that_it_is_read_only`,
      `an_in_flight_count_appears_only_while_something_is_generating`,
      `an_actioned_answer_is_marked_in_the_history`,
      `a_follow_up_renders_indented_under_its_parent`,
      `the_empty_history_pane_says_how_to_start`. One fix came out of
      writing them: the Q&A headline is wider than the pane, and since
      the renderer truncates from the right, the `→ TODO` / `→ session`
      markers were being cut off at every realistic width. They now
      precede the harness/run-mode provenance, so the droppable part is
      what gets dropped.
- [x] Add `src/handlers/learning.rs` and wire it into the main key
      dispatch: navigation, scope toggle, level toggle, selection keys,
      the two ask keys (explain / change), starter-question picker,
      intent flip inside the prompt, answer-pane scrolling, harness
      picker, and `?` help overlay. Dispatch is layered: help, then the
      two pickers, then the question prompt, then the answer pane each
      swallow every key while open, so a stray `q` can't close the
      overlay out from under someone mid-question. Tests:
      `the_help_overlay_opens_closes_and_swallows_keys`,
      `q_closes_the_overlay_but_not_while_a_question_is_being_typed`,
      `tab_submits_the_typed_question`,
      `ctrl_e_flips_intent_without_losing_the_text`,
      `the_starter_picker_fills_the_prompt_without_asking`,
      `the_two_ask_keys_choose_the_intent`,
      `tab_cycles_focus_between_the_three_panes`,
      `selection_keys_change_what_the_question_is_about`,
      `the_level_and_scope_keys_toggle_their_settings`,
      `the_harness_picker_swallows_keys_while_open`. **Follow-up (`Epic
      5`) has no key yet** — it lands with the rest of Epic 5.
- [x] Implement the first-open behaviour: show the help overlay when
      the project's learning session has `onboarding_seen = 0`, then
      set and persist the flag. `learning_show_onboarding_if_new` runs
      at the end of `open_learning_mode`; a missing row or a failed
      lookup counts as "seen", so a DB error can never make the intro
      reappear on every open, and a project with no DB never shows it.
      Persistence is covered by `db::learning::onboarding_flag_is_sticky`.
      **Not yet covered:** an overlay-level test that opens twice and
      asserts the help appears only the first time — the existing
      overlay tests run without a DB, so that needs a DB-backed `App`
      fixture. Worth adding in Epic 6.
- [x] Add the dashboard entry key `K` (verified unbound in
      `handle_normal_key`; the `K` in `handlers/todos.rs` is a different
      mode) on the selected feature/project, register it in
      `DASHBOARD_KEYBINDING_ACTIONS`, and add it to the dashboard help
      overlay list. `App::open_learning_mode_for_selection` resolves the
      selection: a feature or session row opens that feature, a project
      row opens its first feature, and a project with no features says
      why nothing happened instead of swallowing the keypress. Tests:
      `k_opens_learning_mode_on_the_selected_feature`,
      `k_on_a_project_row_opens_its_first_feature`,
      `k_explains_itself_when_the_project_has_no_features`,
      `closing_the_overlay_returns_to_the_dashboard`, and
      `k_makes_the_overlay_actually_render`, which drives the whole
      chain — key sets the mode, `ui::draw` dispatches on it, the
      overlay paints. `L` (prompt library) is unaffected, and
      `default_keys_are_unique_across_actions` now guards the whole
      table against a future collision stealing a binding (the remap
      lookup resolves a pressed key to the *first* matching action, so a
      duplicate would silently shadow one). The config wizard derives
      its list from the same table with no separate label map, covered
      by `the_wizard_offers_every_dashboard_action_including_learning_mode`.
- [x] Implement the starter-question table and picker (anchor-aware
      filtering, loads into the editable prompt). Nine presets in
      `STARTER_QUESTIONS`, scoped `Project` / `File` / `Lines`; a file
      preset still applies once a range inside that file is selected.
      The picker opens the prompt if it isn't already open, so it is a
      way *into* asking, and confirming loads the preset text as
      editable — it never asks on its own. Tests:
      `project_presets_are_offered_only_for_the_project_anchor`,
      `line_presets_need_a_line_or_hunk_range`,
      `file_presets_apply_to_ranges_inside_that_file`, plus the handler
      test above. The list itself is still a guess — see the open
      question below.

### Epic 5 — Acting on an answer

- [x] Implement **ask a follow-up**: new Q&A row with `parent_qa_id`,
      same anchor, parent Q&A embedded in the prompt, rendered indented
      under its parent, with a depth cap that trims the oldest
      ancestors from the prompt beyond the cap. `F` on the Q&A pane or
      inside the answer pane opens the prompt on the selected row;
      following up on a row that hasn't answered yet says to wait rather
      than doing nothing. Two things fell out of building it:
      - **A follow-up must not re-read the cursor.** `learning_ask` took
        its anchor from wherever browsing had left the file list, so a
        follow-up asked after navigating away would quote the wrong
        file. Both the prompt context and the stored row now take an
        `AskAnchor` captured from the parent (`learning_ask_at` /
        `learning_prompt_context_at`), and surrounding-file context is
        only attached while the loaded file is still the one being asked
        about. `learning_submit_question` now honours the capture the
        prompt editor was already making, which it had been ignoring.
      - **A follow-up must land under its thread.** Rows were appended,
        so a follow-up on an older question rendered indented under
        whatever happened to precede it. `thread_insert_index` places it
        past the parent and every descendant.
      Tests: `a_follow_up_carries_its_parents_question_and_answer_into_the_prompt`
      (parent turn appears exactly once),
      `a_two_deep_follow_up_keeps_the_whole_conversation` (both turns,
      oldest first), `a_follow_up_asks_about_its_parents_code_not_wherever_browsing_ended_up`,
      `a_follow_up_lands_under_the_thread_it_continues`,
      `a_thread_insert_lands_past_every_descendant`,
      `following_up_on_an_unanswered_question_says_to_wait`, and the
      handler-level `f_asks_a_follow_up_from_the_answer_you_are_reading`.
      The depth cap itself was already built and tested in Epic 3
      (`follow_up_context_is_capped_at_the_configured_depth`).
- [x] Implement **deep dive**: `D` on the Q&A pane or inside the answer
      pane reruns the selected question through
      `HeadlessRunner::run_read_only`, stored as its own row under the
      original so the first answer survives and the two can be read
      against each other. Three decisions came out of building it:
      - **The answer being checked is not in the prompt that checks
        it.** A deep dive takes the *origin's* position in the thread,
        so it inherits the origin's ancestors but not the origin's own
        turn — a rerun that re-derives the facts is worth more than one
        anchored on the guess it was meant to catch.
        (`learning_deep_dive_context`.)
      - **A rerun preserves the level it reruns**, not the current
        setting, so a pair of answers on the same question reads alike
        even if the user has since switched to `Familiar`. This is why
        `learning_enqueue` now takes level and run mode off the
        `LearningPromptContext` instead of off the live overlay — the
        one place a `learning_qa` row is born, so asking and re-asking
        can't drift apart.
      - **Every refusal says why.** Unanswered rows say to wait; a row
        that already read the repo (including every Codex row, which
        `effective_for` downgrades up front) says so and points at `F`;
        a second deep dive jumps to the one that exists rather than
        paying for it twice, unless the first one failed — which is
        exactly when a retry is wanted.
      Two surface bugs came out of driving the real binary rather than
      the tests, both of them the "silently swallowed keypress" failure
      this mode is supposed to not have:
      - **The footer had no room.** `D` on the first line truncated off
        screen at 140 columns; moving it to the second still pushed
        `q close` off, because `z fold Start here` and `D` *do* coexist
        — the file list only reloads on a scope toggle, so the Start
        here group lingers all session after the first question. They
        now take turns, `D` first, since the group is a leftover by then
        and `z` remains in the `?` overlay. Guarded by
        `the_deep_dive_key_survives_the_footer_at_a_real_width`, which
        asks with the group present and asserts `q close` survives.
      - **The answer pane covered the banner.** `D` pressed on an answer
        that already read the repo set the refusal behind the pane, so
        nothing appeared until the pane was closed — in the one place
        the key is most likely pressed. `draw_answer` now carries the
        banner itself, above its key footer
        (`a_refusal_raised_inside_the_answer_pane_is_visible_there`).
      Tests: `a_deep_dive_re_asks_the_same_question_with_the_repo_readable`,
      `a_deep_dive_does_not_feed_the_shallow_answer_back_to_the_agent`,
      `a_deep_dive_of_a_follow_up_keeps_the_conversation_above_it`,
      `a_deep_dive_keeps_the_level_its_original_was_answered_at`,
      `a_deep_dive_asks_about_its_originals_code_not_wherever_browsing_ended_up`,
      `a_deep_dive_of_an_unanswered_question_says_to_wait`,
      `a_deep_dive_of_a_deep_dive_says_it_already_read_the_repo`,
      `a_second_deep_dive_jumps_to_the_one_you_already_have`,
      `a_failed_deep_dive_can_be_retried`, and the handler-level
      `d_sends_the_answer_you_are_reading_back_with_the_repo_open`.
      **Verified against real Claude**, driving the built binary in tmux
      against a throwaway copy of `~/.config/amf/amf.db`: asked "In one
      sentence, what is this project?" at the project anchor, and the
      no-tools answer **fabricated a `Bash: ls -la && cat README.md`
      tool call and its output** — a `crates/` directory, a
      `rustfmt.toml`, and a README opening "A local-first orchestration
      layer for long-running coding agents", none of which exist. `D`
      re-asked it; the deep dive answered correctly from the real
      README and the real `.worktrees/` mechanism. The row landed
      indented under its parent as `Claude · read the repo`, the
      original kept its answer, the header counted it while it ran,
      pressing `D` on the deep dive itself was refused, and pressing `D`
      on the parent again jumped to the existing row without a third
      run.
      Four review findings on the first cut, all fixed:
      - **A rerun is not a turn in the conversation.** Threading the
        deep dive under its origin made `parent_qa_id` mean two things
        at once, so a follow-up on the verified answer walked straight
        back into the shallow one and handed its (possibly invented)
        evidence to the next prompt as fact. The two relationships are
        now distinct: `deep_dive_of` (`deep_dive_of_qa_id`,
        MIGRATION_021) names the row a deep dive replaced, and
        `learning_ancestor_turns` steps *over* that row to its parent.
        It could not be inferred from `run_mode` — every Codex row is a
        deep dive, follow-ups included, which is also why the
        "already sent that one deeper" lookup now matches on
        `deep_dive_of` instead of parent + run mode.
        (`a_follow_up_on_a_deep_dive_leaves_the_answer_it_replaced_behind`,
        `a_follow_up_on_a_deep_dive_still_carries_the_turns_above_it`,
        `a_codex_follow_up_is_not_mistaken_for_a_rerun`.)
      - **Read-only tools do not make an answer repository-grounded.**
        Both modes sent the same prompt and only the tools differed, so
        a deep dive could answer from the excerpt alone while the row
        claimed it read the repo. `build_prompt` now ends with
        `run_mode_instructions`: the deep dive is required to open the
        file and what it depends on and to name what it checked; the
        no-tools run is told to say what it cannot see rather than fill
        it in. The mode comes off the run that will actually be
        dispatched (`effective_for`), so a downgraded Codex row can't be
        told to answer blind while its label says otherwise.
        (`the_deep_dive_template_requires_the_repository_to_be_read`,
        `the_no_tools_template_says_it_cannot_see_the_rest_of_the_repository`.)
      - **Two refusals described a state the row wasn't in.** `D` on a
        *running* deep dive said to wait for it, though it is refused
        after it lands too; and jumping to an existing deep dive said
        "here is what it came back with" while it was still running.
        Both now branch on `is_in_flight`.
        (`d_on_a_running_deep_dive_says_to_follow_up_not_to_wait`,
        `a_second_deep_dive_while_the_first_runs_says_it_is_still_going`.)
      - **The answer pane's footer outgrew the pane.** Adding `D` took
        it to 114 columns; the pane has ~92 inner columns at a
        110-column terminal, and it truncates from the right, so `Esc
        back to browsing` — the only way out of a modal — was clipped.
        `answer_footer` now fits the line by dropping hints instead,
        rarest navigation first, never `Esc`, and omits `F`/`D`
        entirely on a row that would refuse them.
        (`the_answer_footer_keeps_the_way_out_and_the_actions_when_narrow`,
        `the_answer_footer_only_offers_keys_the_row_can_act_on`.)
- [x] Implement **relabel intent** on an existing entry (explain ⇄
      change), persisted, with the answer text left untouched. `i` on the
      Q&A pane or inside the answer pane; a follow-up asked afterwards
      inherits the new intent, which is what keeps the label from being
      decoration. Three decisions came out of building it:
      - **The banner has to say the answer wasn't rewritten.** A marker
        that changes by itself invites exactly the reading a newcomer
        will make — that the answer below it was regenerated to match.
        The confirmation states the text is unchanged and points at `F`,
        which is the key that actually gets an answer written the other
        way. (`re_filing_says_the_answer_was_not_rewritten`.)
      - **A confirmation must not be painted as a failure.** The overlay
        had one banner channel, `error`, rendered in the danger colour,
        and it was already carrying non-failures ("you already sent that
        one deeper"). Re-filing needed to report success, so
        `LearningViewState::notice` now shares the line in the info
        colour; each channel clears the other, and the notice clears when
        the Q&A cursor moves, since it names one row.
        (`a_confirmation_is_not_painted_as_a_failure`,
        `the_re_filing_banner_clears_when_the_cursor_moves_on`.)
      - **An in-flight row can be re-filed.** The prompt is already
        dispatched under the old framing whether or not it has landed, so
        refusing would withhold the label for nothing — but the banner
        branches so it never describes an answer that doesn't exist yet.
        (`a_question_still_generating_can_be_re_filed`.)
      Action affordances re-order in the answer footer: a change request
      leads with `D`, because a change proposed without the repository
      open is the least trustworthy kind of answer, while an explanation
      leads with `F`. The `i` hint's own label flips with the intent
      ("file as a change" / "file as a note"). Fitting it in cost the
      footer its `g/G top/bottom` hint even at 140 columns, so the drop
      policy became an explicit rank rather than a droppable prefix:
      `g/G` goes, then `PgUp/PgDn`, then `i` itself, and only then `j/k`
      — `F`, `D`, and `Esc` never drop.
      Tests: `re_filing_an_explanation_as_a_change_keeps_its_answer`,
      `re_filing_goes_both_ways`,
      `a_follow_up_after_re_filing_inherits_the_new_intent`,
      `re_filing_with_nothing_asked_says_so`,
      `a_re_filed_entry_reloads_the_way_it_was_filed` (DB-backed),
      `a_re_filed_entry_carries_the_other_marker`,
      `the_answer_footer_leads_with_what_the_entry_kind_makes_likely`,
      and the handler-level `i_re_files_the_answer_you_are_reading`.
      **Verified by driving the built binary** in a 140×44 tmux against a
      throwaway copy of `~/.config/amf/amf.db` with real history: `i`
      flipped the row to `! change` and the pane title to *Ask for a
      change*, the answer text and question were untouched, the footer
      re-ordered to `D` before `F` with the hint reading *file as a
      note*, the banner cleared when the cursor moved to the next
      question, and the new label was still there after closing and
      reopening the overlay. One fix came out of it: the first draft of
      the confirmation was 155 characters and the banner is a single
      unwrapped line, so at 140 columns it lost the "ask a follow-up (F)"
      clause — the one part that stops the new marker implying the answer
      was rewritten. The messages are now short enough to fit, guarded by
      a length assertion in
      `re_filing_says_the_answer_was_not_rewritten`.
      The run is reproducible:
      `scripts/dev/screenshot/scenarios/learning-mode-refile.txt` drives
      the whole path (open, browse, select a range, ask, wait for a real
      headless answer, re-file, help overlay) against a seeded throwaway
      repo, and is the first captured scenario for this feature.
- [x] Implement **make actionable** as an explicit, confirmable action
      on any entry. `a` on the Q&A pane or inside the answer pane opens
      `LearningActionEditor` — an editable title seeded from the answer, the
      note that would be written shown in full, and a statement that this adds
      a note about the code rather than a change to it. `Enter` writes it via
      `AmfDb::add_todo`, `Esc` writes nothing. The editor lives inside
      `LearningViewState` like the pickers, so walking away from it returns to
      exactly the browsing state underneath, and the answer pane stays open
      behind it — keeping an answer is not the start of something else the way
      `F` and `D` are, so the confirmation lands inside the pane you are
      reading. Four decisions came out of building it:
      - **A list nobody can open is worse than no list.** The plan said
        `load_or_create_todo_list`, but a `todo_lists` row with no
        `SessionKind::Todos` session is invisible from the dashboard — the
        note would be written and then unreachable. It takes quick-capture's
        route instead (`add_todos_session_for_picker`, then the list), so the
        session row exists before the first item does.
        (`keeping_an_answer_makes_the_list_reachable_from_the_dashboard`.)
      - **`→ TODO` is a promise the TODOs overlay can stop keeping.** An item
        can be deleted from over there, and jumping into a list that no longer
        holds it is the swallowed keypress this mode is meant not to have. A
        stale link is dropped and a fresh note offered, saying which of the two
        happened. (`keeping_an_answer_whose_item_was_deleted_offers_a_new_one`.)
      - **Without a DB it refuses out loud.** The Q&A history survives in
        memory, but an in-memory TODO would not even be visible from the
        dashboard, so pretending is worse than refusing.
        (`nothing_is_kept_without_a_database`.)
      - **A failed link is reported, not rolled back.** The item exists by
        then; undoing would mean deleting a note the user just watched being
        added, so the banner says the item landed and the link didn't.
      The seeded title strips the markdown an agent's lead line arrives wearing
      (`##`, `**…**`, `-`, `1.`) since a TODO title renders raw, and cuts at a
      word boundary with an ellipsis. An explanation's first line is still a
      guess — which is the whole reason the title is editable and nothing is
      written until it is confirmed.
      The answer pane's footer went to **two lines**, actions above navigation:
      one line of all eight hints wants over 150 columns against the pane's ~92
      at a 110-column terminal, and the drop policy was starting to eat
      `PgUp/PgDn` and `i` at 140. Each line is still fitted independently for
      genuinely narrow terminals.
      Tests: `keeping_an_answer_writes_nothing_until_it_is_confirmed`,
      `a_kept_answer_lands_on_the_projects_todo_list`,
      `the_title_you_type_is_the_one_that_is_written`,
      `a_note_with_no_title_says_so_instead_of_being_written`,
      `keeping_the_same_answer_twice_opens_the_item_you_already_have`,
      `a_kept_answer_is_still_marked_after_a_reopen`,
      `an_answer_that_has_not_arrived_cannot_be_kept`,
      `a_failed_question_says_to_ask_it_again_rather_than_keeping_nothing`,
      `the_confirmation_fits_on_one_line`,
      `a_change_proposals_lead_line_becomes_the_title_without_its_markup`,
      `a_title_skips_blank_and_decoration_only_lines`,
      `an_explanations_title_is_a_truncation_you_can_edit`,
      `a_title_falls_back_to_the_question`,
      `the_note_says_where_in_the_project_it_came_from`,
      `a_long_answer_is_excerpted_into_the_note`, the handler-level
      `a_keeps_the_answer_you_are_reading` and
      `esc_walks_away_from_the_confirmation_without_writing`, and the render
      tests `the_keep_confirmation_says_what_it_will_and_will_not_do`,
      `a_refusal_raised_inside_the_keep_confirmation_is_visible_there`,
      `the_keep_hint_says_whether_it_would_add_or_open`.
      **Verified against real Claude**, driving the built binary in a 140×44
      tmux through
      `scripts/dev/screenshot/scenarios/learning-mode-keep-todo.txt` against a
      seeded throwaway repo: asked about a line range in `src/main.rs`, kept
      the answer, edited the seeded title, and confirmed. The note carried
      `src/main.rs:11-15`, the question, and the answer excerpt; the
      confirmation banner appeared inside the answer pane; the history row
      gained `→ TODO`; the footer hint flipped to *open its TODO item*;
      pressing `a` again opened the TODOs overlay with that one item selected
      ("1 open, 0 done") under a toast saying why the screen had changed; and
      the dashboard showed the new TODOs session. One fix came out of the first
      run: `a` had closed the answer pane the way `F` and `D` do, which left
      `Esc` closing the whole overlay instead of the pane and made the
      jump-back unreachable from where it is actually pressed.
      **Not driven end to end:** spawning an agent from the resulting item.
      The body is stored (asserted above) and `todo_spawn_prompt` appends
      `todo.body` verbatim, so the context reaches the composer, but the launch
      itself was not exercised here — it is the TODOs overlay's existing `g`
      flow, unchanged by this work.
- [x] Implement **escalate to live session**: `S` on the Q&A pane or inside
      the answer pane creates an agent-harness session on the feature via
      `create_agent_session_labeled`, enters it with
      `enter_view_without_auto_compose`, and seeds the composer with
      `open_compose_seeded` — editable, not auto-submitted. The seed
      (`escalation_seed`) carries the anchor, the selection, the question and
      the answer, phrased by intent, with the `Newcomer` overlay asking the
      live agent to narrate. `spawned_session_id` is recorded on the row
      *before* the mode changes, the way the TODO spawn does it, since once
      `enter_view` lands there is no `state.qa` left to write to. Four
      decisions came out of building it:
      - **A toast raised on arrival is never painted.** `ui::dashboard` draws
        the `Compose` branch and `return`s before the shared `draw_toasts`
        pass, so the first cut's "this session can change files" toast — the
        one thing this key most needed to say — was invisible in the one place
        it was raised. Drawing toasts there is not the fix either: they stack
        from the bottom-right, which is exactly where the compose box sits, so
        they would cover the prompt the user is meant to read. The statement
        moved **into the seed**, and the leftovers (a stale link replaced, a
        link that failed to save) moved to `self.message`, which
        `promote_message_to_toast` surfaces the moment the user steps back to
        the pane.
      - **…and it has to be the seed's *last* line, not its first.** The
        composer opens with the cursor after the last line, so the tail is what
        is on screen. Verified the wrong way round first: the opening line
        carrying it scrolled off, and a ~60-line seed meant the user arrived
        looking at the middle of a quoted answer. It is now the closing ask —
        "Unlike the run that produced it, you can change files here — so ask me
        before you change anything" — which is both visible and true of the
        agent reading it.
      - **A failed row is escalatable; an in-flight one is not.** A headless run
        that never came back is precisely when a live agent is worth reaching
        for, so the seed says there was no answer rather than leaving a gap that
        reads as one. An in-flight row is refused, because escalating it would
        set two agents on the same question at once.
      - **The session runs the feature's agent, not Learning Mode's harness.**
        The live session is work on this feature and every other session in it
        runs that agent; continuity costs nothing because the seed carries the
        answer verbatim rather than relying on the agent remembering it.
      A repeat press returns to the linked session without re-seeding — that
      conversation already has this context — and a link whose session has been
      removed is dropped and a fresh one started, saying which of the two
      happened. The answer footer's `S` hint flips between "hand to a live
      agent" and "back to its session"; fitting it cost the label its verbosity,
      since with five hints the pane's ~118 inner columns at 140 leave exactly
      22 for this one, and going over dropped the whole `i` hint.
      Tests: `an_escalated_question_carries_where_what_and_the_answer`,
      `a_shallow_answer_is_handed_over_with_its_limits_stated`,
      `the_seed_asks_for_what_the_entry_was_filed_as`,
      `a_newcomer_seed_asks_the_live_agent_to_explain_itself`,
      `escalating_a_failed_question_says_there_was_no_answer`,
      `a_long_answer_is_excerpted_into_the_seed`,
      `a_diff_selection_is_handed_over_as_a_diff`,
      `the_session_is_labelled_with_the_code_it_is_about`,
      `escalating_opens_a_session_with_the_prompt_filled_in_and_unsent`,
      `a_second_escalation_returns_to_the_session_you_already_have`,
      `escalating_after_the_session_was_removed_starts_a_new_one`,
      `escalating_a_question_still_generating_says_to_wait`,
      `escalating_with_nothing_asked_says_so`, the handler-level
      `s_hands_the_answer_you_are_reading_to_a_live_agent`, and the render test
      `the_escalation_hint_says_whether_it_would_start_or_return`. The
      overlay-level tests needed a first for this feature: a `MockTmuxOps` with
      real launch expectations (`launchable_app_for_handlers`), since this is
      the only Learning Mode action that starts anything.
      **Verified against real Claude**, driving the built binary in a 140×44
      tmux through `scripts/dev/screenshot/scenarios/learning-mode-escalate.txt`
      against a seeded throwaway repo: asked about a line range in
      `src/main.rs`, pressed `S` on the answer, and a real `claude` session
      titled `Learning: src/main.rs:11-15` came up with the composer pre-filled
      and **nothing sent** — the closing boundary line was the last thing on
      screen. `Esc` left the draft intact on the live pane, the dashboard showed
      the new session under the feature, the history row carried `→ session`,
      the footer hint had flipped to *back to its session*, and pressing `S`
      again jumped into that session rather than starting a second.
      Review found two lifecycle gaps in this, both since fixed:
      - **A surviving record is not a live session.** The reuse check only
        looked the session up in the feature, so an agent that had exited (or
        a window killed from tmux) still counted as the linked conversation and
        `S` opened a dead pane. `learning_session_is_reusable` now also asks
        tmux for the window — but only when the feature's tmux session is
        running, because a *stopped* feature is restarted by `enter_view` and
        gets every saved window back. Test:
        `escalating_after_the_agent_exited_starts_a_new_one`.
      - **A failed launch left the record behind.**
        `create_agent_session_labeled` adds the session before the window,
        harness, and save, so a failure partway left a session in the tree with
        no agent behind it while telling the caller "nothing was changed" — and
        with no link recorded, the next `S` added another. The launch is now one
        unit (`launch_agent_session_window`) whose failure kills the window and
        removes the record; a failed *save* is deliberately not rolled back,
        since the agent is up by then, and is logged instead. Test:
        `a_failed_launch_leaves_no_session_behind`.

### Epic 6 — Hardening and docs

- [x] Capture the mode end to end and fix what the capture exposed.
      Thirteen frames in `docs/screenshots/learning-mode/` (scenario
      `scripts/dev/screenshot/scenarios/learning-mode.txt`), driven
      against a throwaway instance seeded with a small demo repo, with
      one real headless Claude run. Reading the rendered answer — rather
      than the code that produces it — is what surfaced both defects
      below, neither of which any unit test would have caught:
      - **The shared markdown renderer dropped whatever opened a list
        item.** `push_inline_text` only appended when a text block was
        already open, and a *tight* list item carries its content with
        no `Tag::Paragraph` around it, so the first inline event had
        nowhere to land: `` `Ok(())` — it worked`` rendered as `• — it
        worked`. Task-list checkboxes (always first in their item) and
        footnote bodies opening with inline code went the same way. The
        block is now opened at that single point rather than by each
        caller remembering to. This bites Learning Mode hardest, because
        a newcomer-pitched answer is written in exactly that shape, but
        the fix is shared with every markdown surface in AMF. Tests:
        `render_markdown_keeps_whatever_opens_a_list_item`,
        `render_markdown_keeps_inline_code_opening_a_footnote_body`.
      - **The answer pane stated its status twice** ("answered by Claude
        · … · answered") and said "answered by" of a row that hadn't
        answered. The status now rides the opening verb alone
        (`answer_provenance`), covered by
        `the_answer_pane_states_its_provenance_once`.
- [x] Close the honesty gaps found by using the finished surface — each
      one a case where the UI stated something the code didn't
      guarantee:
      - **A run interrupted by AMF exiting reloaded as "thinking…"
        forever.** `App::reconcile_interrupted_qa` (called from the
        history load) fails any row that is stored in-flight but has no
        live run in this process, keeping the question so it can be
        asked again; `App::learning_runs_in_flight` is what distinguishes
        a genuinely-still-running row from a stranded one. Covered by
        `a_question_stranded_by_a_previous_run_reloads_as_failed_not_thinking`.
      - **Codex rows claimed "this file only" while reading the repo.**
        `codex exec` has no no-tools mode, so
        `LearningRunMode::effective_for` downgrades the request to
        `DeepDive` *before* the row is written — label, stored row, and
        command now agree. Covered by
        `a_codex_question_is_recorded_as_the_deep_dive_it_will_actually_be`.
      - **A diff selection was quoted with its `+`/`-` markers
        stripped**, so an addition and the line it replaced read as two
        adjacent lines of source. `DiffFile::addressable_line_diff_texts`
        keeps the markers and `LearningViewState::selection_is_diff`
        tells the prompt builder it is looking at a diff. Diff-ness is
        now captured *with* the selection (`AskAnchor`,
        `LearningQuestionEditor`, and the persisted `learning_qa` row)
        rather than re-read at submit time, so a follow-up asked after
        browsing back to the repo tree still labels its parent's excerpt
        correctly. Covered by
        `a_diff_selection_keeps_its_markers_and_says_it_is_a_diff` and
        `a_follow_up_keeps_its_parents_diff_labelling_after_browsing_away`.
      - **Browsing a diff narrowed what the agent could see.** In
        branch-changes scope `learning_load_selected_content` hydrates
        `state.content` from the snapshot's copy of the file, so a
        whole-file anchor is the whole file and a line anchor still has
        surrounding context — the pane keeps addressing diff rows only.
        Covered by `a_changed_file_carries_its_whole_file_not_just_the_hunks`.
      - **An answer that finished after its overlay closed was
        discarded**, leaving the stored row at `running` for good.
        `poll_learning_answers_bg` falls back to
        `finish_learning_qa_in_db` when the row is no longer in the open
        overlay. Covered by
        `an_answer_arriving_after_the_overlay_closed_is_still_saved` and
        `a_failure_arriving_after_the_overlay_closed_is_still_saved`.
      - **A capped repo listing looked complete.** `cap_repo_entries`
        returns the pre-truncation total so the overlay can say how many
        files it is not showing, and point at branch-changes scope.
        Covered by `a_huge_repo_listing_is_capped_and_says_so`.
      - `AmfDb::finish_learning_qa` persists a finished run by id rather
        than rewriting the whole row, so a completion can't overwrite an
        edit made while the answer was generating.
- [x] Add error handling and debug logging (`log_info` / `log_warn` /
      `log_error` with a `"learning"` context) for file load failures,
      headless run failures, and DB errors. User-facing errors must say
      what to do next, not just what failed (e.g. a missing harness CLI
      points at the `A` harness wizard). Confirm no
      `println!`/`eprintln!` was introduced.
      Headless failures (`headless_failure_message` — a missing CLI already
      points at `A`), DB writes, the escalation and TODO paths, and the
      listing failures were already covered as those epics landed. Auditing
      the rest for silence found four gaps, all closed:
      - **Loading a file logged nothing.** The one failure a user meets by
        simply moving the cursor set `content_error` and stopped there, so
        the debug log had no record of *which* file or why. It now logs a
        `learning` warning carrying the path, which the banner itself can't
        give someone reading the log afterwards.
      - **The non-git walk dropped unreadable folders silently.** A
        directory `read_dir` couldn't open was `continue`d past, and a
        listing missing a whole subtree is indistinguishable from a project
        that doesn't have one — the worst shape for a user who doesn't know
        the layout. `walk_files_capped` now returns `RepoWalk { files,
        unreadable }`; each skipped folder is logged by name and the banner
        says how many are missing and where to look. Test:
        `the_fallback_walk_reports_folders_it_could_not_read`.
      - **The onboarding lookup swallowed its error.** `.ok().unwrap_or(true)`
        is the right *behaviour* (an intro that reappears every open is worse
        than one that never shows), but it should not be silent; the failure
        is now logged and the fallback made explicit.
      - **Two messages stopped at what failed.** The file-load errors gained
        next steps, checked against the keys that actually exist in this
        overlay: the project anchor is `P` (not `p`), and there is no reload
        key, so a vanished file points at `s` twice rather than an `r` that
        would do nothing — pointing at a dead key is the swallowed-keypress
        failure this mode exists to avoid.
      An `open` line (`entries`, past-question count, and whether history is
      being saved or is memory-only) and the no-features refusal are now
      logged too, so "`K` did nothing" is diagnosable from the log alone.
      Verified: no `println!`/`eprintln!` in any of the four Learning Mode
      files, `cargo clippy --all-targets` clean, and
      `a_file_that_vanished_says_what_to_do_and_reaches_the_debug_log`
      asserts both halves — the banner's next step and the log entry naming
      the file.
      **Captured** as six frames in `docs/screenshots/learning-mode-errors/`
      (scenario `scripts/dev/screenshot/scenarios/learning-mode-errors.txt`),
      driven against a throwaway instance seeded with a deliberately awkward
      **non-git** project — a mode-000 folder, a mode-000 file, and a binary
      file — so the fallback walk is the listing path under test. As with the
      first capture, reading the rendered output found two defects no unit
      test would have:
      - **The message carried an absolute path.** `Couldn't read
        /tmp/…/scratchpad/demo-notes-app/credentials.env: …` — a workdir
        prefix long enough to push the advice itself onto a fourth line, and
        duplicating what the pane title already says. `load_file_lines` now
        takes the repo-relative label the file list uses; the log line was
        already prefixing it, so the absolute path was redundant in both
        places. Guarded by an assertion that the workdir prefix stays out.
      - **One `Enter` read the file twice.** The duplicated log line is what
        exposed it: moving the cursor already loads the file, so `Enter` was
        re-reading it from disk before shifting focus. It now only changes
        focus — *unless* the previous load failed, where `Enter` is the only
        retry there is. Tests:
        `opening_the_file_already_under_the_cursor_does_not_read_it_again`,
        `opening_a_file_that_failed_to_load_tries_it_again`.
- [x] Add tests covering: DB round-trip of a session plus Q&A rows
      including `intent`, `level`, `parent_qa_id`, `todo_id`, and
      `spawned_session_id`; follow-up cascade on parent delete; the
      no-DB in-memory path; scope toggling; anchor serialization
      (including the project anchor); and that an answered explain
      entry with no follow-up persists and reloads unchanged (the
      default, non-actioned path).
      Most of the list was already standing from the epics that built it
      (`qa_round_trips_every_field`, `answered_explain_entry_reloads_unchanged`,
      `deleting_a_parent_cascades_to_follow_ups`,
      `the_overlay_works_without_a_database`,
      `toggling_scope_switches_to_the_branch_s_changed_files_and_back`,
      `project_anchor_round_trips_without_a_file`). Auditing it against the
      schema found four holes, and the first of them was a real defect:
      - **A reopened history lost its threading.** Rows reload
        `ORDER BY created_at`, but a follow-up is asked *after* whatever else
        was asked in between — and the renderer takes a row's *placement* from
        the list while taking only its *indentation* from `parent_qa_id`. So a
        follow-up came back indented under an unrelated question: exactly the
        defect Epic 5's `thread_insert_index` fixed for the live list, arriving
        by the other door. `thread_rows` now reorders a loaded history through
        that same function, so there is one notion of order rather than two
        that agree until the overlay is closed. Deep dives thread by
        `parent_qa_id` too, so they came along for free. Tests:
        `a_reloaded_thread_keeps_its_follow_ups_under_their_parents`,
        `a_reloaded_deep_dive_stays_with_the_question_it_re_asked`,
        `threading_a_stored_history_gathers_each_conversation`,
        `threading_keeps_a_row_whose_parent_is_gone` (an orphan is kept, not
        dropped — it is the only copy of a question someone asked).
      - **`selection_is_diff` was written but never asserted back**, though
        the plan already records that it cannot be re-derived from the other
        columns. Now asserted in `qa_round_trips_every_field`.
      - **`parent_qa_id`'s value was never checked on reload.** The cascade
        tests prove a follow-up is *reachable* from its parent, which is a
        different claim from it coming back pointing at the right row — and it
        is the second claim the threading above depends on.
        (`a_follow_up_reloads_pointing_at_its_parent`.)
      - **Only two of the four anchor kinds round-tripped.** `File` and
        `Hunk { index }` were untested, and `Hunk` is the one with a payload
        that isn't a line range. (`every_anchor_kind_round_trips`.)
      **Captured** as five frames in
      `docs/screenshots/learning-mode-thread-reload/` (scenario
      `scripts/dev/screenshot/scenarios/learning-mode-thread-reload.txt`),
      driven against a throwaway instance seeded with a small demo repo, with
      three real headless Claude runs. The before/after pair renders the **same
      scratch database** — the pre-fix frame is that database reopened by a
      binary built without `thread_rows`, so the only difference between the two
      images is the row order, and no second set of runs was paid for. The order
      the scenario drives is the only one that shows the defect: a follow-up on
      the *most recent* question is adjacent to its parent either way, which is
      why every earlier capture of this feature missed it.
      Also landed the overlay-level onboarding test Epic 4 deferred to here
      (`the_intro_opens_on_the_first_visit_only`, plus
      `the_intro_stays_shut_when_there_is_nothing_to_remember_it_with`), and
      stated the no-DB contract as an assertion rather than an assumption:
      `without_a_database_questions_still_work_but_do_not_outlive_the_overlay`
      — the overlay answers questions, and nothing pretends they were kept.
- [x] Run `cargo build`, `cargo clippy`, and the test suite; fix all
      warnings introduced by this feature. `cargo build` and
      `cargo clippy --all-targets` are both clean, and the full suite is
      1902 passing / 0 failing.
- [x] Update `README.md` (feature bullet, the dashboard keybindings
      table, and a `### Learning Mode` section written for a first-time
      reader: what it is for, that it never edits files, the two ask
      keys, starter questions, and the newcomer/familiar levels),
      `CLAUDE.md` (new `src/app/learning.rs`,
      `src/handlers/learning.rs`, `src/ui/dialogs/learning.rs`,
      `src/db/learning.rs` under the `## Architecture` sections, plus a
      Learning Mode section describing the explain/change split and the
      level/threading model), and `CHANGELOG.md`.
      **`CHANGELOG.md`** — an `Added` block (since shipped in `v0.36.0`)
      covering `K`, the two ask keys, starter questions, the levels, the
      non-blocking queue, per-project history, harness choice, and the
      five keys that act on an answer: `F` (follow-ups), `D` (deep dive),
      `i` (re-file), `a` (keep as a to-do), and `S` (hand to a live
      session), plus a `Fixed` block for the error-handling pass: the
      incomplete-file-list warning, the file errors that now say what to do
      next, failures being recorded in the debug log, and a reopened history
      keeping its follow-ups with the question they continue. It covers every
      built behaviour and claims nothing that isn't. This docs pass adds a
      `Documentation` block under `[Unreleased]`, following the existing
      precedent for README-only changes, ending on "nothing about AMF's
      behavior changes" — because nothing does.
      **`README.md`** — a bullet under *What AMF does*, `K` in the dashboard
      keybindings table, and *Understand a codebase you didn't write* as the
      **first** user workflow, since it is the one that needs no prior AMF
      knowledge. Written for someone who has not used the mode: what it is,
      that nothing in it changes their files and `S` is the single exception,
      that they do not have to know what to ask (Start here, `t`), the two ask
      keys and the five answer keys as small tables, the newcomer/familiar
      split, and why `D` exists — a no-tools answer can name files that do not
      exist, which is the mode's sharpest edge and is stated rather than
      buried.
      **`CLAUDE.md`** — `learning.rs` added to the `app/`, `ui/dialogs/`, and
      `handlers/` lists, `K` added to the `handle_normal_key` summary, and a
      `### Learning Mode` section after Feature TODOs. It documents the parts
      that are not re-derivable from reading one file: the read-only
      invariant and its one exception; that intent and level shape prompt
      wording only; that run mode comes off `effective_for` so the label and
      the dispatched command agree; and, at most length, that `parent_qa_id`
      and `deep_dive_of_qa_id` are two different relationships — the trap that
      cost a review finding once already.
      Two drift fixes came out of writing it: the plan named the learning
      tables `MIGRATION_017`/`018`, but editor tracking landed first and took
      those numbers, so they are really `019`/`020` (plus `021`), and the
      verification note cited a migration test by a name it does not have
      (`migration_019_upgrades_a_pre_learning_database`). A plan that cites
      the wrong schema version is worse than one that cites none.
- [ ] File follow-up items for the deferred work: anchor-drift
      resolution (commit SHA + snippet or fuzzy match, modeled on
      `App::reanchor_line_comments`), the alternative actionable
      mechanisms (composer seeded and scoped to file/range, inline
      suggested patch like Final Review suggestions), and turning
      "Where to look next" file references into navigable jumps within
      the overlay. Plus one found by building the escalation and **not
      specific to Learning Mode**: `AppMode::Compose` draws and returns
      before `ui::dashboard`'s shared `draw_toasts` pass, so *any* toast
      raised while landing in the composer is silently swallowed —
      including `open_compose_seeded`'s own "Prompt loaded — review and
      send", which the prompt library has presumably never shown either.
      The fix is not simply adding the call: toasts stack from the
      bottom-right, exactly where the compose box is drawn.

### Epic 7 — Browsing a real repository (collapsible file tree)

The file list is a flat, alphabetically sorted list of every path in
the repo (`build_repo_tree_entries`, `src/app/learning.rs`), rendered
into a pane that is 24% of the terminal — about 32 columns at 140. On
this repo that is 500-odd rows where the first screenful reads
`…ude/commands/amf/ai-review.m`, `…e/commands/amf/pr-continue.m`,
`…ude/commands/amf/pr-create.m`: paths truncated to near-identical
stubs, sorted so that dotfile directories come first and `src/` is
dozens of `j` presses away. The user this mode is built for does not
know the repo's layout, and the list that is supposed to teach it to
them instead hides it. A tree shows structure, keeps names readable by
indenting instead of truncating, and lets a newcomer skip whole
subtrees they have no business in yet.

The pieces already exist: `LearningListEntry` is an enum with a
non-selectable collapsible header (`StartHereHeader`), and
`learning_toggle_start_here` already preserves the cursor across a
collapse. This is a second, general case of that, not a new mechanism.

- [ ] Add directory nodes to `LearningListEntry` (path, depth,
      expanded, child count) and build the repo-tree entries as a
      flattened tree rather than a sorted path list, directories before
      files at each level. Keep the **Start here** group pinned above
      it, unchanged. Branch-changes scope keeps its flat list — a
      handful of changed files needs no tree — unless the change count
      makes one worth it, which is a judgment call to make with real
      numbers, not now.
- [ ] Render the tree: indent by depth, show an expand/collapse marker,
      and label each row with the **leaf name only** rather than the
      full path, so a 32-column pane shows `ai-review.md` instead of
      `…ude/commands/amf/ai-review.m`. The header or content pane still
      has to state the selected file's full path, since the name alone
      no longer identifies it.
- [ ] Decide and implement the opening state. Everything collapsed is
      the honest structural view but hides `src/` behind a keypress;
      everything expanded is today's wall of rows with indentation.
      Proposal: collapsed to the first level, plus auto-expanding the
      path to the **Start here** candidates so `src/main.rs` is visible
      on open. Verify against this repo and one much larger.
- [ ] Keys: expand/collapse the node under the cursor, expand/collapse
      all, and jump to the parent directory. `Enter` on a directory
      toggles it; `Enter` on a file keeps loading it. Add them to the
      footer and the `?` overlay in the same spelled-out style, and
      check the footer still fits — this has already truncated `q close`
      and `Esc back to browsing` off the end twice.
- [ ] Revisit the 20,000-entry cap (`MAX_REPO_ENTRIES`). A tree only
      pays for what is expanded, so the cap can move from the listing to
      per-directory expansion, and the "showing the first 20,000"
      warning can become a per-directory one. This is the item that
      makes repo-tree scope usable on a monorepo rather than merely
      capped.
- [ ] Tests: flattening is a pure function over a path list, so cover
      ordering, depth, and collapse/expand round-trips there; plus
      cursor preservation across a collapse (the existing
      `collapsing_the_orientation_group_keeps_the_cursor_on_its_file`
      is the model), a render test asserting a deep file shows its leaf
      name rather than a truncated path, and that selecting a directory
      does not change the question anchor.

## Open questions

- **A no-tools answer doesn't just invent references — it invents
  evidence.** The first real Claude run followed the newcomer template
  closely but pointed "Where to look next" at line numbers and symbol
  names that do not exist (`src/app/state.rs:812`, `LearningState`,
  `is_git_repo`). A later run at the project anchor was worse: with no
  tools available, it **wrote out a `Bash: ls -la && cat README.md`
  call and a plausible fake result**, listing a `crates/` directory and
  a `rustfmt.toml` this repo does not have, under a README line it does
  not open with. Rendered in the answer pane it is indistinguishable
  from a real tool transcript. Deep dive (`D`) corrected it on the
  same question, so the mitigation works — but it is opt-in, and the
  newcomer this mode is for is the least likely to know they should
  press it. Worth considering for v1.1: detecting fabricated tool
  transcripts in a `NoTools` answer, or stating in the prompt that the
  agent has no tools and must say so rather than simulate them. It also
  raises the stakes on the deferred "make Where to look next navigable"
  item: a jump that fails is at least an honest signal.
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
- **Seeded TODO titles from explanatory answers are poor — confirmed.**
  The action template is designed to produce a usable one-line title; an
  explain answer has no such line. The first real run seeded *"The short
  version"* — the answer's opening heading, which names nothing about
  the code. `strip_markdown_decoration` at least keeps the `##` out of
  it, and the title is editable precisely because of this, but the seed
  is a prompt to type rather than something to accept. Options if it
  keeps grating: seed from the question instead for `Explain` entries,
  or ask the agent for a title as part of the answer.
- **Which agent an escalated session runs is a judgment call.** It uses the
  *feature's* agent, not the harness the Learning Mode picker (`m`) chose,
  on the grounds that the live session is work on the feature and every
  other session in it runs that agent — and that the seed carries the answer
  verbatim, so nothing is lost by switching. The opposite reading is
  defensible: a user who deliberately picked a harness to answer their
  questions may expect the same one to continue. Unvalidated either way.
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
  for both. Epic 7 addresses the listing half of this; content loading
  cost is untouched by it.
- **Whether a directory can be a question anchor.** Epic 7 introduces
  directory rows, and "what is everything in `src/app/` for?" is an
  obvious question for exactly the newcomer this mode serves — arguably
  more useful than the whole-project tour. But a directory anchor has
  no selection text, so it would need a different prompt shape (a file
  listing, or a read-only run that goes and looks), and it sits between
  the existing `Project` and `File` anchors rather than beside them.
  Epic 7 assumes directories are navigation only; promoting them to an
  anchor is a separate decision.
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
