# GUI workflow boundaries

Read the relevant section when changing an adapter. Start with the
[architecture map](architecture.md) for shared ownership and dependency rules.

## Prompt library

The desktop prompt-library adapter in `src/gui_prompts.rs` reads the same
source merge as Remote Control and the TUI, and calls the shared search,
placeholder/default and substitution helpers in `src/prompt_library.rs`.
Content keys exclude generated config IDs/dates and include checkout paths;
insertion reloads templates and allowed session targets before returning an
unsent string. `PromptLibraryPanel.tsx` owns only local selection/fill state,
while `App.tsx` appends the handoff to the session draft after a final workspace
cache check. Neither browsing nor preparation launches a process or writes the
library. Library authoring remains a separate workflow.

## Syntax highlighting

`src/gui_syntax.rs` highlights GUI diffs, reviews and the Learning reader
through the TUI's `highlight` service (detection, installed tree-sitter
parsers, cache). It highlights each diff file's whole base and current
contents and sends per-line token spans that must reproduce the line text
exactly, so multi-line strings and comments are classified with full-file
context. Per-file size limits and a per-projection byte budget bound the
first, uncached load. `SyntaxInfo` explains plain-text fallbacks, and
`install` runs the TUI picker's installer on a background thread. The
frontend's `SyntaxCode.tsx` renders spans as `syn-*` classes coloured by the
`--syn-*` tokens in `syntax.css`.

## Prompt overrides

`src/gui_prompt_overrides.rs` adapts the headless-prompt override manager
without touching `App::mode`, so it can sit above an open interview or review
and its pre-call notice. Rows come from the shared registry and
`resolve_template_layered` over the same DB and `amf.json` layers the call sites
read. Each row carries a revision fingerprint of that prompt's stored overrides
and context paths; saves and clears re-resolve the stable context and refuse a
mismatch. Project writes go through `prompts::project::update_in_repo`, which
the TUI manager also uses. It changes only the `prompt_overrides` key and
refuses a malformed config. `precall_target` maps a pending
`AppMode::PromptPrecall` to its prompt, harness and feature/project context.

## Review summary

`gui_review/summary.rs` projects the shared TUI pre-finish summary rows and
calls its suggestion-preparation step after checking the stable workflow,
reviewed changeset and saved progress. The GUI confirms source writes and
keeps review open for another look at changed code. Reading or closing the
summary preserves unsaved editors and never saves progress.

## PR Triage

`gui_pr_triage.rs` drives the TUI's PR Triage modes (`PrPicker`,
`PrReviewLoading`, `PrReview`, `PrInvestigationLoading`) by stable workflow id
and revision. Reads and writes go through `pr_review/github_access.rs`'s
`TriageGithub` boundary, which production backs with the unchanged `gh` calls
and tests replace with an offline fixture. Each step's `gh` reads run without
the GUI handle lock: `plan_begin`/`plan_poll`/`plan_act` name them under the
lock, `PrTriageReads::run` performs them on a blocking thread, and the
`*_prefetched` call applies the results (reading live only what the plan
missed). A pull request list that must be re-read after a failed comment fetch
is left `loading` for the next poll. Investigations and GitHub writes are
two-step: the adapter records the exact prompt or posted body, and confirmation
re-reads the PR (head, metadata or thread) before calling the shared engine.
`try_pr_review_post_reply`/`try_pr_review_toggle_resolve` return failures instead
of the TUI's dashboard reset, so the GUI keeps the pane and draft.
A thread batch fix (TUI `B`) marks several unresolved GitHub comments, builds the
TUI's combined prompt, and sends it to one explicitly chosen agent session. The
preview carries a SHA-256 digest of the PR head, each comment as fetched, the
session and its `destination_revision`; confirmation recomputes it, re-reads the
PR head and refuses any difference, a missing/replaced/stopped session (never
retargeting) or a missing reply-draft store. One reply draft is registered per
comment before sending and all earlier ones are restored if any registration or
the paste fails. At most 50 comments; the TUI's soft ceilings only warn.
Existing-agent fix submission uses the TUI's receipt-prompt builder and stored
provenance, with an exact-prompt confirmation and fresh PR/session checks.
`TmuxOps` delivers a bracketed paste only to an existing running window; sending
never creates an agent. Successful Enter marks Fixing through the shared
persistence path. A failed Enter retires the confirmation while keeping the fix
editor because terminal delivery can be ambiguous. `amf reply-draft` first sends
to the TUI socket, then falls back to the same SQLite request-id capture guard
when that socket is unavailable. The GUI's existing Done-reply engine reads the
returned draft and its fixing-session attribution; no GUI IPC socket or schema
is added.

## PR Review

`gui_pr_review.rs` opens the project's repository directly on the shared Review
PR list, without choosing a feature or resolving the checkout's branch PR.
The GUI's PR Review panel reads the fetched PR revisions, saves file comments
and a summary in the shared SQLite drafts, and previews the exact GitHub
submission before confirming. List/open/post workers retain the TUI's request
identity and head checks; workflow revisions and a saved-draft baseline reject
stale edits and submissions. Project PR Triage uses the same repository scope
and goes directly to its picker without a branch lookup.

## Review checks

`app/review/checks.rs` owns the shared build/test runner: it drains both output
streams while retaining bounded text and terminates/reaps owned processes on
cancellation or close. `gui_review/checks.rs` previews the effective project
command, rechecks confirmation and workflow freshness, and returns transient
results without saving progress or completing the review.

## Review completion

`gui_review/complete.rs` completes a review through the TUI finish engine. The
TUI's `complete_final_review` is now `record_final_review_round` (write the
round, clear progress, record the snapshot, optional PR post) followed by
`dispatch_review_feedback`; the GUI calls the recording half itself, after
rechecking the confirmed check command, suggestion opt-in batch and handoff
session. Like the TUI it applies the saved opt-in batch and reruns the
configured check rather than trusting a transient GUI result. An actionable
round's prompt goes to the reviewed feature's first agent session only when
the reviewer chose handoff: submitted through `deliver_review_prompt` when the
TUI setting submits and the session runs, otherwise returned once (via
`take_completion`) as an unsent composer draft.

## Supervised edits

`app/supervised_edits.rs` owns Vibeless per-edit approval shared by both
interfaces: the hook reply format, its IPC-or-file delivery and the captured
diff loader. `App::read_notification_files` is the one reader of hook
notification files; the TUI scan and `gui_supervised_edits.rs` both use it.
The GUI binds no IPC socket, so it answers only file-fallback requests, by a
stable edit ID and a revision hashed over the request and both captured
copies. It refuses changed, already-answered and abandoned edits and never
recreates a departed hook's files.
Responders hold a shared file lock through validation and delivery and publish
responses without overwriting an existing decision. A failed signal can retry
only that original decision; TUI delivery failures keep the review and feedback
open. OpenCode uses reply paths unique to each change and a waiting lease with
process, session and change ownership, refreshed while its hook polls. The GUI
requires a live owner and a fresh lease before answering OpenCode requests.

## Session picker

`gui_sessions.rs` gives the GUI the rest of the TUI's session picker. VS Code
goes through `App::launch_vscode_window`, the picker's tracked launch (the
`code` CLI is a test seam, `session_ops::vscode_cli`). The feature's
`launched_editors` rows are projected into the sidebar snapshot, and closing
them calls `App::kill_tracked_editors` after checking the rows the user
confirmed. Custom sessions are re-read from the effective `amf.json` and
matched by a config revision. Their `pre_check` runs between two holds of the
GUI lock, and they are created by `App::add_custom_session_identified`, which
shares its window launch with the TUI's add. `with_feature_started_for_add`
stops a feature that an add started, if the add then fails, for terminals,
editors and custom sessions alike.

## Forking

`gui_fork.rs` sequences the shared fork foundation (`fork_snapshot`:
snapshot, transfer, `populate`; `transcript::CapturedContext`;
`WorktreeOps::create_fork_base`) for the desktop. `prepare` captures the
snapshot and exact-session transcript off the GUI lock and stores them under a
single-use token; `confirm` must echo the previewed project, feature, session,
branch and HEAD, re-verifies them, then creates, populates, registers the
feature stopped, runs the worktree hook (cancellable) and only then, if asked,
starts the harness through `ensure_feature_running`. Outcomes (`Created`,
`Cancelled`, `Failed` with the retained checkout) are recorded per token so a
repeat confirm never forks twice. The TUI's `F` path in `app/feature_ops.rs`
shares `fork_snapshot::populate`, `ProjectSettings` and `remove_checkout`.

## Dormancy

`gui_dormancy.rs` lists the shared `app::dormant` scan with the readings
each row was decided on. Its confirmed stop re-checks every selected feature
against those readings and the current scan, then calls the TUI's stop
(`App::stop_feature_reporting`), whose editor cleanup keeps its ownership
rules; the module signals no process itself. Dormant-list deletion shares
the same observation validation before every TODO-disposition/host choice or
deletion. It also checks the confirmed name
and checkout, then calls the existing GUI/TUI deletion engine on that refreshed
store. The confirmation does not touch attention. Terminal attachment records
`last_accessed`, as TUI view entry does.
