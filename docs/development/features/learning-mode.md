# Learning Mode

Implementation notes for agents and contributors. Source is authoritative
when it disagrees; update this file in the same change.

## Learning Mode

A read-only code reader with an agent attached, for someone who did
not write the code in front of them. Built for a newcomer: nothing in
the mode mutates the repository, a blank prompt is never the only
option, and answers are pitched at a first-time reader by default. See
`docs/backlog/learning-mode-plan.md` for the full rationale.

- **Surface:** `AppMode::Learning(Box<LearningViewState>)`, opened with
  `K` on the dashboard (`open_learning_mode_for_selection` — a project
  row opens its first feature). It borrows the Final Review viewer's
  chrome, **not** its state machine, and creates **no `SessionKind`
  row**: Learning Mode is not a session and never appears in the tree or
  switcher.
- **Read-only invariant:** the only path out of it that can change files
  is escalation (`S`), which opens an ordinary agent session and says so
  in the seed. Keep it that way — relaxing it is a scope decision, not a
  convenience patch.
- **Browsing:** `BrowseScope::RepoTree` lists via
  `diff::list_repo_files` (`git ls-files`, with a capped plain walk for
  non-git projects); `BrowseScope::BranchChanges` uses
  `diff::load_snapshot`, the same call the diff viewer makes. In
  branch-changes scope `learning_load_selected_content` still hydrates
  the **whole file**, so an anchor keeps its surrounding context while
  the pane addresses diff rows. Repo-tree scope also pins a **Start
  here** orientation group (existence-checked well-known files plus a
  repo-level tour question) until the project has any history.
- **The repo-tree list is a tree, and `entries` is derived.** Repo-tree rows
  come from `flatten_tree` (pure: path list + `expanded_dirs` → rows,
  directories before files at each level); branch-changes stays flat. The
  authority on what is open is `LearningViewState::expanded_dirs`, **not** the
  `Dir` rows — every tree operation changes that set and rebuilds, so a cursor
  has to be restored by identity (`row_key`), never by index. Two constraints
  are load-bearing: `learning_rebuild_tree` works from the cached `repo_files`
  and must not re-read the repository (expanding a folder cannot cost a `git
  ls-files`), and `default_expanded_dirs` is seeded **once** per overlay
  (`expanded_seeded`) so a reload never re-opens what the user closed. A
  directory is navigation only — `LearningListEntry::path()` returns `None`
  for one, which is what keeps resting on a folder from moving the loaded file
  or the anchor. Size limits live per directory (`MAX_DIR_CHILDREN`, reported
  on the folder's own row); `MAX_REPO_ENTRIES` is now only a memory valve.
- **Anchors:** `LearningAnchor::{Project, File, Hunk, Lines}`; hunks
  exist only in branch-changes scope. The anchor is captured *with* the
  question (`AskAnchor`), not re-read at submit time, so a follow-up
  asked after browsing away still quotes its parent's code.
- **Anchor drift is derived, never stored.** `learning_check_anchor_drift`
  runs once per open (beside `reconcile_interrupted_qa`, which reconciles
  *runs* the same way this reconciles the *code*) and fills
  `LearningViewState::anchor_drift`, a side table keyed by row id.
  `check_anchor_drift` matches the row's `selection_text` against the file
  as it is now — trimmed, blank lines dropped, the stored position checked
  before the whole-file search, so a re-indent isn't movement and a copy
  made elsewhere doesn't unanchor the original. Dropping lines shifts where
  the evidence starts, so `ExpectedBlock::lead_offset` steps the stored
  position past them; without it a selection opening on a blank line reports
  as having slid down by its own whitespace. Two invariants: the row's
  `line_start`/`line_end` are **never rewritten** (they record where the
  question was asked, and keeping them is what lets the verdict be
  re-derived rather than believed once), and *no verdict* is the answer
  for everything there is no evidence to judge — an unreadable file, an
  empty selection, a `File` anchor whose file still exists. "Unreadable"
  includes a file that can't be stat'd at all: the `Gone` verdict is
  `ErrorKind::NotFound` specifically, not `Path::exists()`, which says the
  same "no" to a deleted file and to an unreadable parent directory. A
  diff-sourced selection (`selection_is_diff`) can be reported `Lost` but
  never `Reanchored`: its range comes from `new_line.or(old_line)`, so it
  is not a baseline to measure against. The verdict rides along into
  `escalation_seed` and `todo_body`, which would otherwise send an agent
  to read a location the code has left.
- **Two intents, one history.** `LearningQaIntent::{Explain, Action}` —
  `e` asks for a teaching answer, `c` for a change proposal. Intent only
  shapes prompt framing and affordance ordering, and is re-labelable
  afterwards (`i`) without rewriting the answer.
- **Level:** `LearningLevel::{Newcomer, Familiar}` is a per-session
  setting (`L`), not per-question. It changes prompt wording only — not
  tools, model, or visibility — and each row records the level it was
  answered at, so a reloaded answer explains why it reads the way it
  does.
- **Prompts** are pure functions: `build_prompt` over a
  `LearningPromptContext`, composed from `intent_instructions`,
  `level_instructions`, and `run_mode_instructions`. Run mode comes off
  the run that will *actually* be dispatched
  (`LearningRunMode::effective_for` downgrades every Codex request to
  `DeepDive`, since `codex exec` has no no-tools mode), so the label,
  the stored row, and the command always agree.
- **Execution:** `HeadlessRunner::run(..., restricted = true)` for the
  default answer, `run_read_only` for a deep dive (`D`). Runs are
  non-blocking and several may be in flight: a persistent `mpsc` channel
  owned by `LearningRuns` plus a thread per run, drained by
  `poll_learning_answers_bg()` next to the other `poll_*_bg` calls in
  `src/cli.rs`. An answer that lands after the overlay closed is still
  persisted (`finish_learning_qa_in_db`), and a row left `running` by a
  previous process is failed on load by `reconcile_interrupted_qa`
  rather than reloading as "thinking…" forever.
- **Threading has two distinct relationships.** `parent_qa_id` is a
  follow-up (`F`) — the parent's turn goes into the prompt.
  `deep_dive_of_qa_id` is a rerun (`D`) — the row it replaced is stored
  under it for reading side by side, but `learning_ancestor_turns` steps
  *over* it, so a follow-up on a verified answer never carries the
  shallow one's (possibly invented) evidence forward. Ordering goes
  through `thread_insert_index` for live inserts and `thread_rows` on
  reload, so there is one notion of order rather than two.
- **Acting on an answer:** `a` keeps it as a project TODO (via
  quick-capture's route, so the `SessionKind::Todos` session exists
  before the item does — a `todo_lists` row with no session is
  unreachable), `S` escalates to a live agent session
  (`create_agent_session_labeled` → `enter_view_without_auto_compose` →
  `open_compose_seeded`, editable and unsent). Both record their link on
  the row (`todo_id`, `spawned_session_id`) and a repeat press jumps to
  what exists rather than creating a second; a stale link is dropped and
  the replacement announced.
- **Persistence:** `learning_sessions` + `learning_qa`
  (`MIGRATION_019`, extended by `MIGRATION_020`'s `selection_is_diff`
  and `MIGRATION_021`'s `deep_dive_of_qa_id`), accessed via
  `src/db/learning.rs`. Kept out of the `ProjectStore` JSON like the
  todo tables, with `delete_learning_sessions_for_project` wired into
  project deletion. As with todos, the in-memory list is the overlay's
  source of truth and the mode works without a DB — it just says so
  rather than pretending history was kept.
- **Every refusal says why.** A missing key, a swallowed keypress, or a
  banner that describes a state the row isn't in are the failure modes
  this mode exists to avoid; new actions should state what happened and
  which key to press instead.
