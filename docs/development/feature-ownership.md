# Feature ownership and runtime lifetimes

Start with the [architecture map](architecture.md); use this reference when
changing PR Triage, Final Review, Learning, or their worker ownership.

## Feature modules

| Feature | Files under `src/app/` | Responsibility |
| --- | --- | --- |
| PR Triage | `pr_review/domain.rs` | Comment/review model, attribution, prompt/hunk transforms |
| | `pr_review/fetch.rs` | GitHub normalization, fetch/cache and PR picker coordination |
| | `pr_review/actions.rs` | Selection, marks and action orchestration |
| | `pr_review/reply.rs` | Reply drafting, provenance and posting |
| | `pr_review/investigation.rs` | Read-only investigation requests, results and follow-ups |
| | `pr_review/integration.rs` | Fix target/harness selection, injection and linked session navigation |
| | `pr_review/memory.rs` | Review-memory bootstrap, compact and append workflows |
| | `pr_review/review_tab.rs` | "Review a PR" tab: open-PR list, tab switching, opening a PR into the viewer and leaving it |
| | `pr_review/revisions.rs` | Object-only PR fetch into `refs/amf/review/N/*`, head check, merge-base, ref cleanup |
| | `pr_review/github_access.rs` | `TriageGithub` read/write boundary (`gh` in production, fixtures in tests) and the investigation runner seam |
| | `pr_review/state.rs`, `runtime.rs` | Dialog types and background work ownership |
| Final Review | `review/preparation.rs` | Diff snapshots, persisted progression/history and review notes |
| | `review/progression.rs` | Navigation, selection, approvals, filters and review summary |
| | `review/history.rs` | Read-only round browsing, lazy archive loading and shared current-review projection |
| | `review/comments.rs` | Anchors, comments, suggestions and editor operations |
| | `review/headless.rs` | Walkthrough/co-review workers, completion and feedback dispatch |
| | `review/checks.rs` | Shared build/test check execution, bounded output and process ownership |
| | `review/pr_drafts.rs` | PR review drafts in SQLite: save, resume, PR-update flags and outdated comments |
| | `review/pr_submit.rs` | PR review submit dialog, GitHub payload and posting |
| | `review/state.rs` | Viewer/comment/undo/history state and mode-owned child handles |
| Learning | `learning/lifecycle.rs` | Open/close, reload, settings and persistence coordination |
| | `learning/navigation.rs` | File trees, anchors, selection and answer navigation |
| | `learning/workers.rs` | Question prompts, execution and answer application |
| | `learning/follow_up.rs` | TODO creation and agent-session handoffs |
| | `learning/state.rs`, `runtime.rs` | Session/display types and persistent answer tracking |

Feature roots expose consumed entrypoints and compatibility re-exports. Free
helpers are imported from their owning sibling module; new private helper access
is bounded to the original feature. Extracted modules import explicit App types.
Pure transformations take data rather than an all-purpose App/context object.
State/domain code does not import handlers or renderers. Orchestration may use
existing cross-feature APIs for TODO/plan/session handoffs.

The neighboring `ai_review.rs`, `review_destination.rs`, `review_memory.rs` and
`triage_feature.rs` retain their existing responsibilities. Extraction does not
absorb them just to shrink a file. Shared DiffScope/layout concepts remain
available to ordinary diff browsing rather than becoming private to Final Review.

## Runtime ownership and stale results

Runtime groups establish concrete boundaries with private fields and shared
production/test defaults. The historical [field inventory](baseline/app-fields.tsv)
is a refactor baseline, not the current App definition:

- `PrReviewWork`: independent fetch/investigation receivers; begin, poll and
  cancel methods. Cancel drops the receiver without killing the spawned worker.
  AppMode still determines whether a result has a live target. The "Review a
  PR" list, open, and post slots tag each result with a request id so a retry
  or a closed tab drops a stale one; their blocking loaders are plain `fn`s,
  swapped in tests. A finished post is recorded even with no live target.
- `AiReviewRun`: receiver, pending origin and live progress. Completion or PR
  invalidation clears them together; closing the running screen preserves them
  for later result application and reopening. Existing PR/workdir/head matching
  stays in feature orchestration, including successor invalidation.
- `LearningRuns`: persistent answer channel and in-flight question IDs. Delivery
  retires its matching ID, then orchestration updates the visible matching row
  or the original DB row when the overlay has closed/switched. Closing the
  overlay does not cancel the worker or erase its pending identity.

Final Review already owns its children in mode state; it keeps that boundary.
Shared routing/store/configuration,
manager injection, IPC/watcher/observer guards, pane/sidebar queues and debug/perf
remain on App. Review-memory jobs, refresh caches and cross-feature notification
state remain available for later incremental ownership work.
