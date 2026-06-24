# PR Comment Review

- **Status:** Backlog
- **Owner:** unassigned
- **Relates to:** `trigger_final_review` / `DiffViewer` mode
  (`src/app/review.rs`), embedded tmux view (`AppMode::Viewing`,
  `src/app/view.rs`), compose box / prompt injection
  (`src/app/compose.rs`), prompt library injection seam
  ([prompt-library-plan.md](prompt-library-plan.md)), `gh` usage in
  PR skills (`scripts/amf/pr-info.sh`, `.claude/commands/amf/pr-*`),
  SQLite store (`src/db/store.rs`), `ClaudeLauncher::run_headless`
  (`src/claude.rs`)

## Why / problem

When a PR comes back from review (human reviewers **and** bots like
CodeRabbit/Copilot), the user has to leave AMF, open GitHub, read each
comment, context-switch back into the agent session, paste the relevant
bit, and repeat — often dozens of times per PR. It's tedious and it
burns agent tokens because each round-trip re-establishes context.

AMF already owns the agent session and the working tree. It should let
the user **triage every PR comment in one place** and, per comment,
either (a) inject a tightly-scoped fix prompt into the live agent
session, (b) post an AI-drafted reply (e.g. "done in `<sha>`" or "not
needed because…"), or (c) skip. Because review back-and-forth is
high-volume, **token efficiency is a first-class design constraint**:
the TUI does all fetching and triage itself (zero agent tokens), and
the agent is only paid for the work the user explicitly asks for.

## Decisions captured (from design interview)

- **Sources:** GitHub inline review comments, review summaries
  (Approve / Request changes / Comment bodies), issue/PR conversation
  comments, and bot reviews. Bots are listed **inline like any
  reviewer** (no special grouping), but their boilerplate is **stripped
  before any text reaches the agent**.
- **Agent action on "fix":** inject a prompt into the **live tmux
  session** (reuse warm context; don't cold-start headless).
- **UI:** a **dedicated full-screen review pane** (sibling of the
  embedded tmux view / diff viewer), list on the left, detail + actions
  on the right.
- **Token strategy:** fetch comment **metadata first via `gh` in the
  TUI, hydrate full bodies on demand**. Fix prompts carry **comment +
  GitHub-provided diff hunk + `file:line` only** — the agent opens the
  file itself if it needs more.
- **Reply behavior:** AMF **posts replies to GitHub via `gh`**, but
  always **AI-drafts then waits for user approval/edit before posting**.
  Thread *resolution* is an explicit, optional action — not automatic.
- **Entry point:** **auto-detect the PR** for the selected feature's
  branch, with a **manual PR-number override**.
- **State:** **hybrid** — GitHub thread resolution is the source of
  truth for done/not-done; local notes (skip reasons, drafts, "handled"
  flags) are cached in **SQLite**.
- **Fix loop:** **manual confirm, no auto-advance** — user watches the
  agent, then marks done / posts a reply, then moves on.
- **Refresh:** **manual key only** — no background polling.

## Token-efficiency design (the core constraint)

This is the part to get right. Principles, in priority order:

1. **The TUI spends zero agent tokens on fetching, listing, or
   triage.** All comment retrieval, parsing, grouping, dedup,
   boilerplate-stripping, and resolution state is done in Rust via
   `gh api`. The agent is invoked *only* for fix prompts and (optional)
   reply drafting.
2. **Metadata-first, hydrate on demand.** The list view needs only
   `path`, `line`, `author`, `is_bot`, `is_resolved`, and a one-line
   snippet. Full bodies / threads are loaded lazily when a comment is
   selected. (Network-cheap; also keeps the model tiny.)
3. **Minimal fix-prompt context.** A "fix" prompt =
   `comment body (stripped)` + `diff_hunk` (GitHub already returns this
   per inline comment — free, no extra read) + `file:line` pointer +
   one instruction line. **No file contents are injected**; the agent
   already has the repo and opens what it needs. This is the single
   biggest token lever.
4. **Reuse the warm session.** Injecting into the live agent pane reuses
   accumulated context instead of `run_headless` re-reading the codebase
   from cold on every comment.
5. **Strip boilerplate before sending.** Bot comments wrap content in
   `<details>`, collapsible "prompt for AI agents" blocks, signatures,
   and badges. Strip these (and quoted diffs the agent already has) so
   only the actionable sentence reaches the prompt.
6. **Don't send resolved/handled threads to the agent at all.** Triage
   filters them out before any injection.
7. **Collate threads; send the leaf, not the chain.** A reply thread is
   one item; inject the root comment + latest reply, not every
   intermediate message.
8. **Batch is opt-in, not default.** Default loop is one comment at a
   time (manual confirm). A later phase can offer "queue several fixes
   → one numbered prompt" that shares file context across items to cut
   repeated preamble tokens.
9. **Cache by `PR# + head SHA`.** Persist the fetched/normalized comment
   set in SQLite keyed on the PR head commit. Re-opening the pane is a
   cache hit (no `gh` calls, no tokens). Manual refresh re-fetches.
10. **Cheap drafting path.** AI-drafted replies are short and
    structured (comment + what changed → 1–3 sentence reply). Use
    `run_headless` with a compact prompt, or a smaller/faster model, so
    drafting doesn't cost a full session turn. Drafting is skippable —
    the user can type the reply directly.

## GitHub data sources (all via `gh`, in Rust)

| Need | Call |
| --- | --- |
| Resolve PR for branch | `gh pr view --json number,headRefOid,url` |
| Inline review comments | `gh api repos/{o}/{r}/pulls/{n}/comments` (gives `path`, `line`, `diff_hunk`, `body`, `user`, `in_reply_to_id`, `pull_request_review_id`) |
| Review summaries | `gh api repos/{o}/{r}/pulls/{n}/reviews` (`state`, `body`, `user`) |
| Conversation comments | `gh api repos/{o}/{r}/issues/{n}/comments` |
| Thread **resolution** state + resolve action | GraphQL `pullRequest.reviewThreads { isResolved, id, comments }` and `resolveReviewThread` mutation (REST doesn't expose resolution) |
| Post a reply | `gh api ... /pulls/{n}/comments/{id}/replies` (inline) or issue comment endpoint (conversation) |

Resolution state is the one piece REST can't give us, so a single
GraphQL query maps `comment id → thread id → isResolved`. That mapping
also powers the optional "resolve thread" action.

### Preconditions: `gh` installed, authenticated, scoped

Everything in this feature shells out to `gh`, so before we resolve a
PR or fetch anything, AMF must verify the environment and **fail fast
with an actionable message** rather than surfacing raw `gh` errors deep
in the flow. Check, in order, the moment the user invokes the
PR-review action (cheap, runs once per entry):

1. **`gh` on PATH** — mirror the existing tmux/claude startup checks
   (`TmuxManager::check_available`, `ClaudeLauncher::check_available`).
   If missing → "GitHub CLI (`gh`) not found. Install it from
   https://cli.github.com to use PR review."
2. **Authenticated** — `gh auth status` (exit code is the signal; don't
   parse human text). If not logged in → "`gh` is not authenticated.
   Run `gh auth login`." Suggest the `! gh auth login` in-session escape
   hatch since login is interactive and can't run headless.
3. **Repo has a GitHub remote / PR resolvable** — `gh pr view --json …`
   on the feature's branch. Distinguish the failure modes:
   - no GitHub remote / not a GH repo → explain manual-PR-number won't
     help here;
   - remote exists but **no open PR for this branch** → offer the
     manual PR-number override directly;
   - network/API error → show it and let the user retry/refresh.
4. *(Optional, deferred)* **token scope** — replies/resolution need
   write scope. Don't gate read-only triage (Epic A) on this; only
   check/handle a 403 lazily when the user first tries to **post**
   (Epic C), with a "needs `repo` scope — run `gh auth refresh -s repo`"
   message.

Implementation notes: do these checks in Rust (zero agent tokens),
cache the install/auth result for the session (don't re-run `gh auth
status` on every keystroke), and route every message through
`show_error` so it lands in the debug log too. Treat a `gh` upgrade
prompt or stderr noise as non-fatal.

## Proposed design

### Data model

```text
PrReview {
  pr_number: u32,
  head_sha: String,          // cache key
  url: String,
  repo: (owner, name),
  comments: Vec<PrComment>,
  fetched_at: DateTime,
}

PrComment {
  id: u64,                   // GitHub comment id
  thread_id: Option<String>, // GraphQL review thread (for resolve)
  kind: Inline | ReviewSummary | Conversation,
  author: String,
  is_bot: bool,
  path: Option<String>,      // None for conversation/summary
  line: Option<u32>,
  diff_hunk: Option<String>, // free context from GitHub
  body_raw: String,          // lazy / full
  body_snippet: String,      // one line for the list
  in_reply_to: Option<u64>,  // thread collation
  is_resolved: bool,         // from GraphQL (source of truth)
  // local-cached triage:
  local: TriageState,        // Untriaged | Fixing | Done | Skipped | Replied
  local_note: Option<String>,// skip reason / draft
}
```

GitHub resolution is authoritative for done/not-done; `TriageState` +
`local_note` are the SQLite-cached local layer (skip reasons, drafts,
"I injected a fix" before the thread is resolved).

### SQLite

One new table, e.g. `pr_comment_triage(pr_number, comment_id,
head_sha, state, note, updated_at)`, plus a cache blob table
`pr_review_cache(pr_number, head_sha, json, fetched_at)`. Follows the
existing migration pattern in `src/db/store.rs`.

### New app mode & files

- `AppMode::PrReview(PrReviewState)` — mirrors the `DiffViewer` /
  `Viewing` precedent (a state struct with a `selected` index, a
  `loading` variant `PrReviewLoading` for the initial fetch).
- `src/app/pr_review.rs` — fetch (spawn `gh`), normalize, strip
  boilerplate, cache, triage actions, prompt assembly, reply posting.
- `src/ui/dialogs/pr_review.rs` (or a `draw_pr_review_view` in
  `ui/dashboard.rs`) — the full-screen pane.
- `src/handlers/pr_review.rs` — key handling for the pane.
- `src/github.rs` — **reusable, feature-agnostic** `gh` tool-manager
  (`GhCli`), peer to `TmuxManager` / `WorktreeManager` /
  `ClaudeLauncher`. Owns preconditions and typed PR/issue/review
  queries so other features can reuse it. Feature-specific normalization
  (boilerplate stripping, thread collation) lives in `app/pr_review.rs`,
  not here.

### Injection seam

Reuse the **prompt-library / compose** injection path
(`src/app/compose.rs`): a "fix" assembles the minimal prompt and
delivers it to the feature's live agent window (compose box if
intercept is on, else paste-without-send), so the user reviews before
it runs. This is the same `deliver_prompt` seam the prompt library
uses — no new injection mechanism.

## Rough UI design

**List + detail (default view):**

```text
┌─ PR #321 · Enable prompt composer · 7 comments (4 open) ──────────────┐
│ Comments                          │ src/app/sync.rs:42  ·  @alice      │
│ > [ ] src/app/sync.rs:42  @alice  │────────────────────────────────────│
│   [ ] dashboard.rs:88 @coderabbit │ diff hunk:                         │
│   [x] README.md:10  @bob  ✓done   │   38   poll_interval = 250;        │
│   [-] review summary  @alice       │   42 + self.sync_statuses();       │
│   [ ] (conversation)  @carol       │                                    │
│                                    │ @alice:                            │
│   ─ 3 resolved (h to show) ─       │ "This can race with the poller —   │
│                                    │  guard it behind the lock."        │
│                                    │                                    │
│                                    │ thread: 1 reply ▸ (enter to expand)│
│ j/k move  h hide-resolved          │                                    │
│ f fix  r reply  n not-needed       │ [f]ix [r]eply [n]ot-needed [s]kip  │
│ R resolve  o browser  ↻ r refresh  │ [R]esolve [o]pen  [enter] thread   │
└──────────────────────────────────────────────────────────────────────┘
```

Legend: `[ ]` untriaged, `[x]` done, `[-]` skipped, `✓done` = resolved
on GitHub. Bots shown inline (`@coderabbit`) with no special grouping.

**Reply-draft dialog (AI-draft → approve before post):**

```text
┌─ Reply to @alice · src/app/sync.rs:42 ───────────────────────────────┐
│ AI draft (edit freely, then ⏎ to post, esc to cancel):               │
│                                                                       │
│ Good catch — wrapped the sync call in the existing state lock in      │
│ <sha>. The poller can no longer observe a half-updated status.        │
│                                                                       │
│ [⏎] post & (R) also resolve   [d] re-draft   [esc] cancel            │
└───────────────────────────────────────────────────────────────────┘
```

**Fix confirmation (what gets injected — token preview):**

```text
┌─ Inject fix into agent session ──────────────────────────────────────┐
│ Will send to amf-prompt-composer ▸ claude:                            │
│                                                                       │
│ Address this PR review comment.                                       │
│ File: src/app/sync.rs:42                                              │
│ Comment (@alice): "This can race with the poller — guard it behind    │
│ the lock."                                                            │
│ Diff hunk:                                                            │
│   42 + self.sync_statuses();                                          │
│                                                                       │
│ ~120 tokens · no file contents included                               │
│ [⏎] inject   [e] edit   [esc] cancel                                  │
└───────────────────────────────────────────────────────────────────┘
```

## Workflow

1. From the dashboard, with a feature selected, press the PR-review key
   → AMF resolves the PR via `gh pr view` on the branch (or prompts for
   a number on manual override) and enters `PrReviewLoading`.
2. AMF fetches (3 REST + 1 GraphQL), normalizes, strips bot
   boilerplate, dedups threads, caches by head SHA → `PrReview` view.
3. User triages top-down. Per comment:
   - **f (fix):** assemble minimal prompt → confirm/edit → inject into
     the live agent session. Comment marked `Fixing`. **No
     auto-advance** — user watches the agent.
   - **r (reply):** AI-draft → user edits → post via `gh`. Marked
     `Replied`.
   - **n (not-needed):** AI-draft a "why not needed" reply → edit →
     post. Marked `Skipped` with the reason as the local note.
   - **s (skip):** local-only, optional note.
   - **R (resolve):** optional explicit GraphQL `resolveReviewThread`.
   - **o:** open the comment in the browser.
4. After the agent finishes a fix, user returns, optionally posts a
   "done in `<sha>`" reply (and/or resolves), marks done, moves on.
5. **r (refresh)** re-fetches on demand (e.g. after pushing fixes and a
   re-review). No background polling.

## Progress

### Epic A — Read-only triage MVP (no GitHub writes, no agent)

- [x] Preconditions: check `gh` on PATH (`GhCli::check_available`) and
      `gh auth status` (`GhCli::check_auth`); resolve PR for the branch
      (`GhCli::resolve_pr`) distinguishing no-PR / no-remote / other,
      with actionable messages. _(Per-session caching of the auth result
      and the manual-override entry are wired in with the entry point.)_
      → `src/github.rs`.
- [x] `gh` wrappers: resolve PR from branch + by number; fetch inline
      comments, review summaries, conversation comments (paginated via
      `--paginate --slurp`, flattened). → `src/github.rs`.
- [x] GraphQL query for thread resolution state → `comment_id → thread`
      (paginated, `GhCli::review_threads`). → `src/github.rs`.
- [x] Normalize into `PrReview`/`PrComment`; `is_bot` detection;
      bot-boilerplate stripper; one-line snippets; resolution merge;
      `fetch_and_normalize` orchestration (Rust, unit-tested). _(Thread
      `thread_id` is attached per comment; full reply-chain collation for
      display lands with the UI.)_ → `src/app/pr_review.rs`.
- [x] `AppMode::PrReview` + `PrReviewLoading`; spawn fetch off the UI
      thread (background `std::thread` + channel, polled via
      `poll_pr_review_bg` in the main loop); full-screen loading frame
      with cancel. → `src/app/pr_review.rs`, `src/app/state.rs`,
      `src/handlers/pr_review.rs`, `src/main.rs`.
- [~] Full-screen list+detail pane: list (resolution marker, location,
      author, snippet) + detail (header/flags, diff hunk, body) shipped
      with `j/k` navigation. _Remaining:_ lazy body hydration,
      hide/show-resolved toggle, scrolling. → `src/ui/dialogs/pr_review.rs`.
- [~] Dashboard entry key: `G` auto-detects the branch's PR (runs
      preconditions → resolve → load) and is listed in help. _Remaining:_
      manual PR-number override prompt. → `src/handlers/normal.rs`.
- [ ] SQLite cache keyed by `PR# + head SHA`; manual refresh key.
- **Acceptance:** open any PR for the current branch and read every
  comment inside AMF, grouped and navigable, with zero agent tokens
  spent and a cache hit on re-open.

### Epic B — Fix injection into the live session

- [ ] Minimal fix-prompt assembler (comment + diff_hunk + file:line);
      token estimate shown.
- [ ] Confirm/edit dialog; deliver via the compose/prompt-library
      injection seam to the feature's agent window.
- [ ] Local `TriageState` persisted in SQLite (`Fixing`/`Done`/etc.);
      manual "mark done" with no auto-advance.
- **Acceptance:** select a comment, inject a scoped fix into the warm
  agent session, watch it work, mark done — without leaving AMF and
  without injecting any file contents.

### Epic C — Replies & resolution (GitHub writes)

- [ ] AI-draft reply via `run_headless` (compact prompt / small model),
      skippable.
- [ ] Approve/edit dialog → post reply via `gh` (inline replies +
      conversation comments).
- [ ] "Not-needed" flow = drafted reply + local skip note.
- [ ] Optional explicit `resolveReviewThread`; refresh affected thread
      after posting.
- **Acceptance:** post an AI-drafted, user-approved reply to a thread
  and optionally resolve it, all from the pane.

### Epic D — Throughput & polish

- [ ] Opt-in batch mode: queue several "fix" decisions → one numbered
      prompt sharing file context.
- [ ] Filters/sort (open-only, by file, by author, humans-first).
- [ ] "Done in `<sha>`" reply template auto-filled from latest commit.
- [ ] Keybinding help entry; status-bar summary (`4 open / 7`).
- [ ] Token usage surfaced per session (tie into `token_tracking.rs`).
- **Acceptance:** a 30-comment bot-heavy PR can be triaged quickly with
  measurably lower token spend than copy-paste round-trips.

## Nice to have

- **Triage-session token/cost tracker.** Surface a running tally of
  tokens, usage, and estimated `$$` spent during a PR-comment triage
  session — so the user can see, in-pane, exactly what the review loop
  is costing. Builds on the Epic D "token usage surfaced per session"
  item (`token_tracking.rs`), but scoped specifically to the PR-review
  pane: count tokens spent on fix injections and reply drafts, attribute
  them to the PR (`PR# + head SHA`), and show a live total in the pane
  header or status bar (e.g. `~3.2k tok · ~$0.04 this session`). Because
  triage is intentionally zero-token for fetch/list/triage, this makes
  the "only pay for the work you asked for" design constraint visible and
  auditable. Stretch: per-comment cost breakdown and a per-PR cumulative
  total persisted in SQLite, so re-opening a PR shows total spend to
  date.

- **Active-PR indicator on the dashboard.** Show a marker next to a
  feature in the dashboard list when its branch has an open PR — e.g. a
  small badge or icon, ideally with the PR number and unresolved-comment
  count (`PR #321 · 4`). Makes it obvious which features have a PR worth
  reviewing (and how much is outstanding) before pressing `G`, and turns
  the review entry point into something you're nudged toward rather than
  having to remember. Must stay cheap: resolve PR state in the
  background (reuse the `GhCli` layer) and cache it per `branch + head
  SHA` so the dashboard never blocks or spams `gh`; refresh on the
  existing status-sync cadence rather than per-frame. Stretch: dim/hide
  the badge once all threads are resolved, and color it by review state
  (changes-requested vs. approved vs. comments-only).

## Open questions

- **Reply identity:** replies post as the user's `gh` auth. Fine, but
  should AMF tag AI-drafted replies (e.g. a subtle footer) for honesty?
- **Drafting model:** dedicated small/fast model for reply drafts vs.
  the feature's configured harness — config knob?
- **Resolution without reply:** GitHub lets you resolve without
  commenting; keep `R` independent of `r` (current design) or always
  prompt to leave a note?
- **Multi-line / outdated comments:** comments on lines that have since
  changed (`line: null`, outdated). Show with a clear "outdated" badge;
  diff_hunk still gives context.
- **Non-GitHub remotes:** GitLab/Bitbucket explicitly out of scope for
  v1 (GitHub `gh` only).
- **Conversation vs. review threads:** conversation comments have no
  `path`/resolution — group them in a separate "Conversation" section
  of the list?

## Reasoning / when to build

Build after the prompt-library injection seam is stable (Epic B depends
on it). Epic A is independently valuable and low-risk (read-only, no
agent, no writes) — a good first slice to ship and validate the fetch /
normalize / strip pipeline before wiring in any agent or GitHub writes.
The token discipline (metadata-first, minimal fix prompts, warm-session
reuse, caching) is what makes high-volume review loops affordable and is
the reason to do this in AMF rather than in the agent chat directly.
