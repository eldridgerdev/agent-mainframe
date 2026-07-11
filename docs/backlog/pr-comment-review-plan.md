# PR Comment Review

- **Status:** Backlog
- **Owner:** unassigned
- **Relates to:** `trigger_final_review` / `DiffViewer` mode
  (`src/app/review.rs`), embedded tmux view (`AppMode::Viewing`,
  `src/app/view.rs`), compose box / prompt injection
  (`src/app/compose.rs`), prompt library injection seam
  ([prompt-library-plan.md](prompt-library-plan.md)), `gh` usage in
  PR skills (`scripts/dev/amf/pr-info.sh`, `.claude/commands/amf/pr-*`),
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
- **Agent action on "fix":** inject the prompt into a tmux agent
  session. **Default: spin up (and reuse) one dedicated review session**
  for all of the PR's fixes — pays per-session overhead once and
  amortizes file reads across comments (most token-efficient for
  multi-comment PRs). **Option: reuse the feature's existing live
  session** when the user prefers warm in-progress context. Never a new
  session per comment. (See the "which agent session" decision under
  Open questions.)
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
4. **One persistent session for all fixes — never one-per-comment.**
   Default to a dedicated review session reused across every fix so the
   per-session overhead (system prompt, tool definitions, skills) is paid
   once and file reads are amortized across comments. A fresh session per
   comment re-reads the same files cold and repeats that overhead N
   times. Reusing the feature's existing live session is offered as an
   option (warm context) but isn't the default. Either way, keep the
   agent alive instead of cold-starting `run_headless` per comment.
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

**Epic E keys** layer onto this same pane: `A` runs an AI review of the
diff (its findings appear as draft `[ ]` items in this same list, triaged
with the existing verbs), and `M` appends the selected comment's finding
to `.amf/review-memory.md`. The lookback bootstrap (distill the memory
from the last *N* PRs) is reached from the PR entry flow:

```text
┌─ Bootstrap review memory ────────────────────────────────────────────┐
│ Distill common findings from recent PRs into .amf/review-memory.md.   │
│ Look back over:                                                       │
│   ( ) 20 PRs     (•) 50 PRs     ( ) 100 PRs     ( ) all               │
│                                                                       │
│ Comment fetch is free (gh); one agent pass distills → ~8k tokens.     │
│ [⏎] run   [esc] cancel                                                │
└───────────────────────────────────────────────────────────────────────┘
```

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
- [x] Full-screen list+detail pane: list (resolution marker, location,
      author, snippet) + detail (header/flags, diff hunk, body) with `j/k`
      navigation, `h` hide/show-resolved toggle (navigation skips hidden
      comments; selection snaps off a now-hidden row; "N resolved hidden"
      indicator), and detail scrolling (`^d`/`^u`, `PgUp`/`PgDn`, clamped to
      content; resets on selection change). _Lazy body hydration is moot —
      the REST list fetch already returns full comment bodies, so there's no
      second round-trip to defer._ → `src/ui/dialogs/pr_review.rs`,
      `src/handlers/pr_review.rs`, `src/app/pr_review.rs`, `src/app/state.rs`.
- [x] Dashboard entry key: `G` auto-detects the branch's PR (runs
      preconditions → resolve → load) and is listed in help. When no open PR
      is found for the branch, AMF opens a manual PR-number override prompt
      (`AppMode::PrNumberPrompt`): digit-only input, `Enter` resolves via
      `gh pr view <n>` and starts the fetch, resolve failures show inline so
      the user can correct and retry. The prompt is also reachable on demand
      with `g` from inside the review pane (review a different PR than the
      branch's auto-detected one). → `src/handlers/normal.rs`,
      `src/handlers/pr_review.rs`, `src/app/pr_review.rs`, `src/app/state.rs`,
      `src/ui/dialogs/pr_review.rs`.
- [x] SQLite cache keyed by `PR# + head SHA` (`pr_review_cache` table, migration
      008; `PrReview`/`PrComment`/`PrRef` made `Serialize`/`Deserialize`). Opening
      a PR whose head commit is unchanged is a cache hit — zero `gh` calls, instant
      pane (`enter_pr_review`); a miss falls back to the background fetch and writes
      the result to the cache on completion. Manual refresh key (`r`) re-resolves
      the PR (picking up a new head SHA after a push) and re-fetches, bypassing the
      cache and overwriting the row. Stale rows (>7 days) are evicted at startup
      alongside the token cache. → `src/db/pr_review_cache.rs`,
      `src/db/migrations.rs`, `src/app/pr_review.rs`, `src/github.rs`,
      `src/handlers/pr_review.rs`, `src/ui/dialogs/pr_review.rs`.
- **Acceptance:** open any PR for the current branch and read every
  comment inside AMF, grouped and navigable, with zero agent tokens
  spent and a cache hit on re-open.

### Epic B — Fix injection into an agent session

- [x] Minimal fix-prompt assembler (`PrComment::fix_prompt`): one instruction
      line + `file:line` pointer + bot-stripped comment text + GitHub diff hunk,
      with **no file contents** injected (token principle #3). Outdated comments
      get a "line has since changed" note; conversation/summary comments omit the
      `File:` line. Token estimate via `estimate_tokens` (~4 chars/token) for the
      confirm-dialog "~N tokens" hint. Unit-tested. → `src/app/pr_review.rs`.
- [x] Fix-target session strategy: **spin up and reuse one dedicated
      review session by default**; offer "reuse the feature's existing
      live session" as an option. Reuse the existing one for the whole
      PR; never one session per comment. The pane carries a `FixTarget`
      (`DedicatedReview` default / `ExistingLive`), toggled with `t` and shown
      in the footer (`f fix→dedicated`). Targeting is resolved by
      `fix_session_index` (find-or-create the dedicated session by the stable
      `"PR Review"` label; reuse the first agent window for existing-live) and
      `create_dedicated_review_session` spins one up (project's preferred agent,
      feature's mode/flags) on first fix, reused thereafter — never one per
      comment. Pressing `f` resolves the target, ensures the feature is running,
      and injects the minimal fix prompt via the shared compose/prompt-library
      seam (paste-without-send), switching the user into that session to watch.
      Unit-tested. → `src/app/pr_review.rs`, `src/app/session_ops.rs`,
      `src/app/state.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`.
- [x] Confirm/edit dialog; deliver via the compose/prompt-library
      injection seam to the chosen agent window. Pressing `f` now opens a
      modal confirm/edit dialog (`FixConfirmState` on `PrReviewState`) seeded
      with the comment's minimal fix prompt instead of injecting immediately:
      it names the target session, shows the exact prompt that will be sent
      (no file contents) and a live `~N tokens` preview via `estimate_tokens`,
      and lets the user edit the buffer (`e` to edit, `esc` back to confirm)
      before `⏎` injects through the existing `deliver_prompt`
      (paste-without-send) seam or `esc`/`q` cancels. The prompt editor reuses
      the shared `TextEditor` / `editor_lines` rendering. Unit-tested. →
      `src/app/pr_review.rs`, `src/app/state.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`.
- [x] Local `TriageState` persisted in SQLite (`Fixing`/`Done`/etc.); manual
      "mark done" with no auto-advance. New `pr_comment_triage` table (migration
      009) keyed by `PR# + comment id + head SHA`, with a `TriageState`
      `as_db_str`/`from_db_str` encoding (unknown tokens degrade to `Untriaged`).
      Triage is **authoritative in its own table**, not the review cache blob:
      `apply_persisted_triage` overlays it onto every freshly-loaded review (both
      the cache-hit and background-fetch paths) so state survives re-open and
      restart. Injecting a fix (`f`) marks the comment `Fixing` and persists
      before leaving the pane; `m` toggles `Done`↔`Untriaged` and `s` toggles
      `Skipped`↔`Untriaged` — both **manual with no auto-advance** (selection
      stays put so the user can watch the agent). The list shows a per-comment
      `[ ]`/`[~]`/`[x]`/`[-]` checkbox and the detail header a colored
      `[fixing]`/`[done]`/`[skipped]` chip, distinct from GitHub's `✓`
      resolution marker. Stale rows (>7 days) evicted at startup. Unit-tested
      (db roundtrip, head-SHA keying, state encoding). → `src/db/migrations.rs`,
      `src/db/pr_comment_triage.rs`, `src/db/mod.rs`, `src/app/pr_review.rs`,
      `src/app/mod.rs`, `src/handlers/pr_review.rs`, `src/ui/dialogs/pr_review.rs`.
- **Acceptance:** select a comment, inject a scoped fix into the review
  session (default dedicated, or the live session by choice), watch it
  work, mark done — without leaving AMF and without injecting any file
  contents.

### Epic C — Replies & resolution (GitHub writes)

- [~] ~~AI-draft reply via `run_headless` (compact prompt / small model),
      skippable.~~ **Dropped after review.** A free-form AI draft of an
      arbitrary reply isn't useful in practice — the replies that matter are
      tied to a triage decision and carry information the model doesn't have
      (the *reason* a fix isn't needed, or the *commit* that fixed it). Replies
      are now **two contextual templates**, both deterministic (no agent
      tokens): a "Done in `<sha>`." report and a "not needed" explanation (see
      the two items below). If a freer drafting path is ever wanted, it should
      assist *those* flows (e.g. phrasing a not-needed reason), not generate
      arbitrary replies.
- [x] Approve/edit dialog → post reply via `gh` (inline replies +
      conversation comments). The shared reply substrate: an editable dialog
      whose `⏎` posts — the one outward-facing action, gated on explicit
      confirm. Inline comments reply into their thread via its root comment
      (`GhCli::reply_to_review_comment`); conversation comments and review
      summaries post a new conversation comment (`GhCli::post_issue_comment`).
      A first-write 403 maps to an actionable "run `gh auth refresh -s repo`"
      message. Driven by the two contextual reply kinds below (`ReplyKind`)
      rather than a free-form entry. → `src/github.rs`, `src/app/pr_review.rs`,
      `src/app/state.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`.
- [x] "Not-needed" flow = reply + local skip note. `n` opens an empty reply in
      edit mode for the user to explain *why* a fix isn't needed; posting marks
      the comment `Skipped` and keeps the explanation as its local note
      (persisted). The reason is the user's, not an AI guess. Unit-tested. →
      `src/app/pr_review.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`. _(Also delivers the Epic D "Done in `<sha>`"
      template: `R` seeds a reply from the feature workdir's latest commit and,
      on post, marks the comment `Done`.)_
- [x] Optional explicit `resolveReviewThread`; refresh affected thread
      after posting. `x` toggles the selected comment's thread between resolved
      and reopened via the GraphQL `resolveReviewThread` /
      `unresolveReviewThread` mutation (`GhCli::set_thread_resolved`), kept
      independent of replying (resolve without commenting). Only inline comments
      that belong to a thread are resolvable — conversation comments / review
      summaries (no `thread_id`) show a hint. On success the new state is applied
      to every comment in that thread and the SQLite cache is refreshed so a
      cache-hit re-open reflects it. Posting a reply now re-pulls thread
      resolution (`refresh_thread_resolution`, one GraphQL call, zero agent
      tokens) so the `✓` marker stays honest. A first-write 403 maps to the
      actionable `gh auth refresh -s repo` message. Unit-tested (mutation parse +
      GraphQL-error surfacing). → `src/github.rs`, `src/app/pr_review.rs`,
      `src/handlers/pr_review.rs`, `src/ui/dialogs/pr_review.rs`.
- **Acceptance:** from the pane, reply to a comment to report a fix
  (`Done in <sha>`) or explain why one isn't needed, and optionally resolve the
  thread.

### Epic D — Throughput & polish

- [x] **Pane clarity & comment readability.** The detail pane is no longer a
      flat wall of lines: it renders as distinct sections — a header with the
      `file:line`, the diff hunk, the body, and any local triage note —
      separated by subtle full-width dividers, with the unfocused detail side
      given a muted border so focus reads as being on the list. The diff hunk
      is colored like a diff (added `+` green / removed `-` red / `@@` headers /
      muted context) **and the code after each marker is syntax-highlighted**
      via the shared tree-sitter highlighter
      (`crate::highlight::highlight_source`, keyed off the comment's file path
      for language detection; the added/removed sides are reconstructed and
      highlighted with multi-line context, then matched back per hunk line). It
      degrades to plain marker coloring when no language is detected (e.g. a
      conversation comment with no path) or a parser isn't installed, and the
      add/remove signal survives either way. Comment bodies render through the
      in-app Markdown renderer (`crate::markdown::render_markdown` — headings,
      lists, code blocks, inline code, wrapped to the pane width) instead of raw
      text, and the local note (skip reason / "not needed" explanation) is now
      surfaced in its own section. Author / role (bot vs. human) / kind /
      resolution / outdated / triage are shown as compact `[label]` chips, and a
      two-line footer adds a marker legend (`✓` resolved, `[outdated]`,
      bot/human, the triage checkboxes). The detail-scroll clamp now bounds
      against the line count the renderer actually drew
      (`PrReviewState::detail_content_lines`, written each frame) rather than a
      hand-synced source-line estimate that drifted once the body became
      Markdown. →
      `src/ui/dialogs/pr_review.rs`, `src/app/state.rs`, `src/app/pr_review.rs`,
      `src/highlight/mod.rs`.
- [x] **Syntax-installer `i` shortcut in the review pane (parity with the diff
      viewer).** `i` in the review pane opens the shared syntax-language picker for
      the **selected comment's** file. `handle_pr_review_key`
      (`src/handlers/pr_review.rs`) maps `KeyCode::Char('i')` to
      `open_syntax_language_picker_for_selected_diff_file`, which gained an
      `AppMode::PrReview` arm (`src/app/syntax.rs`): it pulls the selected
      comment's `path`, computes the `syntax_notice_for_path` hint, and stashes the
      `PrReviewState` as the picker's `return_to` so closing drops the user back
      into the same pane and selection. Conversation/summary comments with no path
      are a no-op (the mode is restored unchanged). Because the review detail
      re-highlights every draw and the picker already clears the highlight cache
      via `reload_runtime_state()` on completion, a freshly-installed parser is
      picked up automatically on return — no per-pane cache to invalidate.
      Discoverability: `i syntax` added to the pane footer key hints and a new
      "While reviewing PR comments" section in the keybinding help
      (`src/ui/dialogs/help.rs`); the diff-hunk section shows an inline
      `<Lang> highlighting not installed — press i` hint (via a new
      `syntax_install_hint` reusing `language_install_state_for_path` /
      `HighlightInstallState::Available`) when the hunk's language is recognized
      but its parser isn't installed. Unit-tested (picker opens on Rust file +
      stashes `return_to`; no-op for a pathless comment). →
      `src/handlers/pr_review.rs`, `src/app/syntax.rs`, `src/ui/dialogs/pr_review.rs`,
      `src/ui/dialogs/help.rs`, `src/app/tests.rs`.
- [x] **Mark PR comment review as experimental in the UI.** The dashboard help,
      manual PR-number prompt, loading pane, review pane header, and status bar
      now label the PR comment-review flow as experimental, matching the way
      AMF labels other still-refining workflows. →
      `src/ui/dialogs/help.rs`, `src/ui/dialogs/pr_review.rs`, `src/ui/status.rs`.
- [x] **PR picker — list PRs to choose from, or enter a number.** A new
      `AppMode::PrPicker` (peer to `PrNumberPrompt`) shows a scrollable, full-screen
      list of the repo's PRs (`#number · title · @author · branch`, newest-updated
      first) so the user can open one without knowing its number. Fetched in Rust
      via `GhCli::list_prs` (`gh pr list --json
      number,title,author,headRefName,updatedAt,isDraft,state`, zero agent tokens;
      `parse_pr_list_json` flattens the nested author login and sorts by
      `updatedAt`). Reached from both entry paths: pressing `G` on a branch with no
      auto-detectable PR opens the picker instead of jumping straight to the number
      prompt, and `g` inside the review pane opens it seeded on the current PR.
      `⏎` resolves the highlighted PR by number (existing resolve → load → cache
      path), `a` toggles open-only vs. include closed/merged (re-fetch, keeping the
      highlight on the same PR when it survives), and `#`/`g` falls through to the
      manual number prompt so **pick-a-PR and type-a-number live behind one entry**.
      If `gh pr list` fails outright the picker degrades to the number prompt so the
      user is never stuck. Draft/merged/closed rows carry a `[chip]`. Footer, status
      bar, and a new "In the PR picker" keybindings-help section list the keys.
      Unit-tested (`parse_pr_list_json` sort + author/null handling). →
      `src/github.rs`, `src/app/pr_review.rs`, `src/app/state.rs`,
      `src/handlers/pr_review.rs`, `src/handlers/mod.rs`, `src/ui/dialogs/pr_review.rs`,
      `src/ui/dialogs/mod.rs`, `src/ui/dashboard.rs`, `src/ui/status.rs`,
      `src/ui/dialogs/help.rs`.
- [x] **Pick the agent harness before the dedicated review session starts.**
      The first `f` of a PR, for the `DedicatedReview` target, now opens a
      single-select harness picker (`HarnessPickState` overlay on
      `PrReviewState`, peer to `fix_confirm`/`reply`) before the fix confirm
      dialog — so PR triage can run on a different harness than the feature's
      working session (e.g. a cheaper/faster model for mechanical fixes). The
      picker lists the repo's allowed agents (`allowed_agents_for_project_path`)
      and highlights the project's `preferred_agent` by default (`j/k` move, `⏎`
      choose, `esc` cancel/abort the fix). The choice is remembered for the rest
      of the PR in `PrReviewState::review_harness` and threaded through
      `resolve_fix_session` → `create_dedicated_review_session(.., harness)`
      (which falls back to `preferred_agent` when `None`); the session is created
      once and reused, so subsequent fixes skip the picker. Re-opening a PR whose
      dedicated session already exists also skips it (the running session's
      harness is inherited — `pr_review_needs_harness_pick` checks
      `fix_session_index`). Only the dedicated target prompts; `ExistingLive`
      reuses whatever harness that session already runs. A bespoke single-select
      modal (`draw_harness_pick`) is used rather than the multi-toggle
      enable/disable `HarnessSetup` dialog, which doesn't fit single selection.
      Keybinding-help note and unit tests (picker opens on first fix + default
      highlight; skipped once a harness is chosen; cancel aborts without
      injecting). → `src/app/pr_review.rs`, `src/app/session_ops.rs`,
      `src/app/state.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/ui/dialogs/help.rs`,
      `src/app/tests.rs`.
- [x] **Upgrade the fix confirm/edit dialog editor (vim + full editing).**
      The dialog added in Epic B seeded a plain `TextEditor::new(prompt)` and
      forwarded keys to it only in edit mode; it now has the niceties the other
      editor-backed dialogs already have. **Vim keymap support** via `Ctrl+T`
      (`TextEditor::toggle_vim` — the app-wide vim-toggle key, matching the
      compose box, steering prompt, and diff-feedback editors), with the choice
      **persisted on the pane**
      (`PrReviewState::fix_vim_enabled`, the same approach as
      `PlaceholderFillState::vim_enabled`) so it survives the editor being rebuilt
      when the dialog is reopened for another comment — a new `new_fix_confirm`
      helper seeds `with_vim`/`new` from that flag. The active keymap shows in the
      dialog **title** (`· vim insert`/`· vim normal`) and the footer hints adapt
      (under vim, `Ctrl+Q` returns to the confirm view since `Esc` is consumed for
      Insert→Normal; in plain mode `Esc` does it). **Scroll + cursor-follow** for
      prompts taller than the dialog reuse `editor_view::sync_editor_scroll` (a
      `scroll`/`sync_to_cursor` pair on `FixConfirmState`, `Ctrl+J/K` and
      `PgUp/PgDn` to scroll, a scrollbar once it overflows) instead of the old
      non-scrolling `Paragraph`; edits/cursor moves re-follow the cursor. The
      standard `TextEditor` affordances (undo/redo, word motions) come for free.
      Submit gesture that coexists with multi-line editing: **`Tab` injects** from
      edit mode (where `Enter` is a newline), while `Enter` still injects from the
      confirm view. Keybinding-help note and unit tests (vim toggle persists
      across reopen; scroll moves the offset and stops following the cursor). →
      `src/app/state.rs` (`FixConfirmState`, `PrReviewState`),
      `src/app/pr_review.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/ui/dialogs/help.rs`, `src/app/tests.rs`.
- [x] **Fix several comments in one pass against the same dedicated session.**
      The throughput loop. `space` toggles a **batch mark** on the selected
      comment (`PrReviewState::marked`, a `HashSet<u64>` keyed by comment id so
      marks survive the hide-resolved filter), shown as a leading `●` in the list
      and a marked-count in the footer (`F fix-marked(N)`). `F` then **queues
      every marked comment's fix prompt into the one review session, in list
      order, without leaving the pane** — each is pasted-and-submitted
      (`C-u` → `paste_text` → `Enter`, with a short delay between so the harness
      registers each as its own turn), so the prompts queue while the agent works
      and the user keeps triaging. Each stays a **separate** prompt (distinct from
      the combined-prompt batch below), sharing the session's warm file context
      (token principle #4); already GitHub-resolved marks are skipped (principle
      #6). Each queued comment is marked `Fixing` and persisted, and the marked
      set is cleared. To avoid auto-submitting into a not-yet-ready agent, `F`
      **requires the review session to already exist** — the first `f` establishes
      and warms it; the batch refuses (with a hint) rather than cold-starting one.
      Keybinding-help + legend entries; unit-tested (toggle add/remove; empty and
      no-session hints; a two-comment queue sends `C-u`/paste/`Enter` ×2, marks
      both `Fixing`, clears the set, and stays in the pane). →
      `src/app/state.rs`, `src/app/pr_review.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/ui/dialogs/help.rs`, `src/app/tests.rs`.
- [x] **Combined-prompt batch: "fix all of these, then I'll come back."**
      The walk-away workflow. `B` builds **one numbered prompt** from every
      marked, not-yet-resolved comment (`space` to mark, reusing the sequential
      item's `marked` set) — a single shared preamble followed by a `Comment N:`
      entry per comment, each carrying the same minimal context as a single fix
      (`file:line` pointer, bot-stripped text, diff hunk) and, like it, **no file
      contents** (`combined_fix_prompt` reuses the new shared `fix_prompt_body`).
      It reuses the existing fix confirm/edit dialog — same `~N tokens` preview,
      editing, vim, and scroll — carrying a `FixConfirmState::batch: Option<Vec<u64>>`
      that flags the combined case (the dialog title becomes "Inject combined fix
      for N comments"). On inject it delivers the one prompt into the dedicated
      review session via the shared compose seam and switches the user in to
      launch-and-leave (send-and-leave, distinct from `F`'s N separate
      auto-submitted prompts): **every included comment is marked `Fixing` and
      persisted, and the marked set is cleared**, so the next refresh reconciles
      what got resolved. First fix of a dedicated-review PR still picks the
      harness first — the batch flow stashes a `pending_batch` flag so the
      picker's continuation reopens the *combined* dialog rather than the
      single-comment one. Bounded by soft ceilings
      (`BATCH_COMBINED_COMMENT_WARN` / `BATCH_COMBINED_TOKEN_WARN`): past either,
      a warning toast fires but the action still proceeds. Footer
      (`B combine(N)`) + keybinding-help entries; unit-tested (combined-prompt
      numbering/preamble; nothing-marked and all-resolved hints; dialog carries
      both ids and excludes resolved/unmarked comments; harness-pick routes back
      to the batch). →
      `src/app/pr_review.rs`, `src/app/state.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/ui/dialogs/help.rs`, `src/app/tests.rs`.
- [x] **File-level comments reference the file, not the whole hunk
      (token fix — from real use).** Comments left on a *file* rather than a
      line dumped that file's entire diff into both the detail pane and the
      assembled fix prompt — a large, low-value token cost against principle #3,
      compounding across the combined batch (`B`) where several oversized hunks
      land in one prompt. `GhCli` now captures **`subject_type`** from the
      inline-comments API (`ReviewComment::subject_type`), and `normalize` sets a
      new `PrComment::file_level` from it, so the classification is reliable
      rather than a length heuristic. A single `PrComment::prompt_hunk()` decides
      what the hunk is worth: `None` for a file-level comment (or, as a backstop,
      one whose `diff_hunk` exceeds `WHOLE_FILE_HUNK_LINES`), `Some(hunk)`
      otherwise. Both `fix_prompt` and `fix_prompt_body` route through it, so the
      single and combined prompts both **carry only a `File: <path>` reference**
      (plus a short "diff hunk omitted — open the file for context" note, so the
      agent knows context was withheld rather than absent) and the agent opens
      the file itself. The detail pane renders the same way — "comment on file
      `<path>`" in place of the wall of diff. The backstop is set **well clear of
      ordinary comments**: sampling real PRs, line-anchored hunks reach ~90 lines
      at the tail (most under 30), so a tighter cap would have stripped the
      context the reviewer pointed at; only a pathological whole-file-sized hunk
      trips it. Also fixes a latent mislabel — a file-level comment has no `line`
      by definition, which `normalize` had been badging as `[outdated]`.
      `file_level` is `#[serde(default)]` so pre-existing `pr_review_cache` rows
      still deserialize. Unit-tested (file-level prompt omits the hunk; a 93-line
      hunk is kept while an over-cap one is dropped without being called
      file-level; the combined batch drops whole-file hunks but keeps ordinary
      ones; normalize sets `file_level` and *not* `outdated`). →
      `src/app/pr_review.rs`, `src/github.rs`, `src/ui/dialogs/pr_review.rs`,
      `src/db/pr_review_cache.rs`.
- [x] **Filters/sort (open-only, by file, by author, humans-first).**
      `hide_resolved` (`h`) already covered "open-only"; this adds an
      independent **sort order**, cycled with `o`: fetch order → by file →
      by author → humans-first (bots last) → back to fetch order. A new
      `PrSortMode` (`src/app/pr_review.rs`) is stored on `PrReviewState`
      (`src/app/state.rs`) and applied by `visible_indices()`, which now
      filters (unaffected) then stably sorts, so ties keep their original
      fetch-order relative position (e.g. two comments by the same author, or
      two on the same file). Comments with no path (conversation/summary)
      always sort last under `ByFile`. Because a custom sort order is no
      longer monotonic in the underlying comment index, `j`/`k` navigation
      (`pr_review_select_next/prev`) was switched from raw index comparisons
      to walking by **position within the current visible order**, and the
      `h` snap-off-a-hidden-selection logic now finds the old selection's
      nearest neighbor via a new `all_sorted_indices()` (the same sort order,
      ignoring the resolved filter) rather than assuming ascending indices —
      both keep working under any sort mode, not just the default. Footer
      (`o sort→<mode>`) and keybinding-help entry. Unit-tested (cycling wraps
      through all four modes; by-file grouping/ordering; by-author stability
      within ties; humans-first keeps bots last while preserving relative
      order within each group; hide-resolved snap uses the active sort
      order). → `src/app/pr_review.rs`, `src/app/state.rs`,
      `src/handlers/pr_review.rs`, `src/ui/dialogs/pr_review.rs`,
      `src/ui/dialogs/help.rs`, `src/app/tests.rs`.
- [x] "Done in `<sha>`" reply template auto-filled from latest commit. Shipped
      with the Epic C reply work: `R` seeds a reply with the feature workdir's
      short `HEAD` (falling back to "Done." outside a git repo), editable before
      posting; on post the comment is marked `Done`. → `src/app/pr_review.rs`.
- [x] Keybinding help entry; status-bar summary (`4 open / 7`). Turned out to
      already be covered, incrementally, by other Epic D items rather than
      needing its own change: the pane header has shown `N comments (M open)`
      since Epic A (`draw_pr_review`'s header line), and every key added since
      (sort, batch fix/combine, `P` toggle, `i` syntax) registered itself in
      the "While reviewing PR comments" keybindings-help section as it landed.
      The dashboard's bottom status bar (`src/ui/status.rs`) has a `PrReview`
      arm too, but it's dead code for this mode — `ui/dashboard.rs::draw`
      returns early for `AppMode::PrReview` before ever reaching
      `status::draw` (same shape as the dead `Viewing` arm noted under the `P`
      toggle item above), so there was nothing to wire up there.
- [x] **Token usage surfaced per session (tie into `token_tracking.rs`).** The
      pane header now shows what triage has spent on the fix-target session
      once it exists — e.g. `· dedicated usage 12.3k eff · $0.15` — reusing
      the same `format_feature_token_usage`/`aggregate_token_usage` helpers
      and `Session::token_usage` field the dashboard's per-feature token badge
      already uses (`src/ui/list.rs`), so it's the existing sync-populated
      number rather than a new tracking path. A new read-only
      `App::pr_review_fix_session_usage()` (`src/app/pr_review.rs`) resolves
      the same session `fix_session_index`/`resolve_fix_session` targets
      (dedicated or existing-live, by `state.fix_target`) but — unlike
      `resolve_fix_session` — never creates one, so it's safe to call on every
      frame just to render a number; it's `None` until the first `f` spins up
      the target session. `ui/dashboard.rs` resolves it once per frame (before
      the `&mut app.mode` borrow) and threads it plus `&app.config.token_pricing`
      into `draw_pr_review`. Unit-tested (`None` before any session exists;
      reads the target session's `token_usage` once one does). →
      `src/app/pr_review.rs`, `src/ui/dialogs/pr_review.rs`,
      `src/ui/dashboard.rs`, `src/app/tests.rs`.
- [x] **Quick toggle between the review pane and the dedicated review
      session (usability — from real use).** Today `f` switches the user
      *into* the dedicated `"PR Review"` session to watch the agent, but
      getting *back* to the pane means exiting to the dashboard and
      re-entering with `G` — losing the scroll/selection and paying a
      re-resolve. In practice the user bounces between "watch the agent
      work" and "triage the next comment" constantly, and that round-trip
      is the friction. `P` **does double duty as the same key on both sides**
      of the round trip, one press each way: in the review pane
      (`pr_review_toggle_to_session`) it jumps into whichever session `f`
      currently targets (dedicated or existing-live, via the same
      `fix_session_index`/`REVIEW_SESSION_LABEL` lookup `resolve_fix_session`
      uses) and stashes the pane's exact `PrReviewState` — selection, detail
      scroll, any open fix/reply/harness dialog — on a new
      `App::pr_review_return: Option<PrReviewReturn>` field rather than on the
      target mode itself (the syntax picker's `return_to: Option<Box<AppMode>>`
      pattern doesn't fit here since the round trip is `PrReview → Viewing →
      PrReview`, and `ViewState` is rebuilt fresh by `enter_view`/the session
      switcher on every hop); from the Viewing side, `leader+P`
      (`pr_review_return_to_pane`) pops the stash back with no re-fetch. Unlike
      `f`, `P` **never creates** the dedicated session as a side effect of a
      peek — if none exists yet it hints "press f to start one" rather than
      spinning one up. The Viewing-side `leader+P` only fires when the
      *current* Viewing session's tmux session/window still matches the one
      `P` jumped to (`PrReviewReturn::session`/`window`, captured from the
      feature's `tmux_session` and the target session's `tmux_window` at jump
      time); navigating elsewhere first — a different feature, a different
      session/window — leaves the stash alone rather than popping an unrelated
      PR's pane into view, and a mismatched or absent stash shows a toast
      instead of silently no-op'ing. Both footers surface the pairing: the
      review pane's key-hint line gains `P session`, and a top-right badge
      (`Ctrl+Space P: back to review`) appears in the Viewing-mode session
      view — next to the existing remote-control/direct-input badges in
      `ui/dashboard.rs`'s `draw()` — whenever the current session/window
      matches a live stash. (`ui/status.rs`'s `AppMode::Viewing` arm looked
      like the natural spot for this but turned out to be dead code: the real
      Viewing-mode draw path returns early before ever reaching
      `status::draw`.) Keybinding-help entries in both the "While viewing" and
      "While reviewing PR comments" sections; unit-tested (`P` in the pane
      requires an existing session rather than creating one; jumps and stashes
      the exact selection/scroll; `leader+P` restores and consumes the stash;
      a session/window mismatch leaves the stash untouched instead of
      restoring into the wrong view; `leader+P` with no stash is a no-op with
      a message). Confirmed live: built the binary, drove it end-to-end in an
      isolated tmux server against the repo's own `#343` `[TEST] PR-review
      pane fixture` PR — `P` jumped into the real dedicated session (badge
      appeared), `leader+P` popped back to the exact selected comment/mark,
      and a second round trip still held.

      **Two follow-ups from real use, same day.** (1) The peek key was
      renamed from `v` to `P` — same key both directions, one press each way,
      more discoverable than two different letters for one round trip. (2)
      **The bigger gap:** `f` (inject fix) — the far more common way into the
      fix session, `P` is just a peek — didn't stash anything, so `leader+P`
      had nothing to restore after the ordinary fix flow. `pr_review_inject_fix`
      now stashes exactly like `P` does, right after marking the targeted
      comment(s) `Fixing` and before handing off to `enter_view_without_auto_compose`.
      Since compose interception is on by default, `f` actually lands in the
      **compose box** (seeded with the fix prompt), not bare `Viewing` — the
      real return path is `f` → `Ctrl+Space` (cancels the compose box back to
      `Viewing`, per `handlers/compose.rs`) → `P`. Caught a second bug during
      live testing of that exact path: the stashed state still carried the
      now-already-actioned `fix_confirm` dialog, so `leader+P` reopened the
      same "inject fix" dialog instead of the plain comment list — fixed by
      clearing `state.fix_confirm = None` before stashing. Both bugs are
      unit-tested (`pr_review_inject_fix_also_stashes_return_state` goes
      through the real confirm-dialog path via `pr_review_open_fix_confirm`,
      not the no-dialog fallback, so it actually exercises the dialog-leak
      case) and reconfirmed live end-to-end against PR #343: `f` → confirm →
      `Ctrl+Space` → `P` now returns cleanly to the comment list with the
      `[~]`/`[fixing]` triage mark intact. → `src/app/state.rs`
      (`PrReviewReturn`), `src/app/mod.rs`, `src/app/pr_review.rs`,
      `src/handlers/pr_review.rs`, `src/handlers/view.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/ui/dashboard.rs`,
      `src/ui/dialogs/help.rs`, `src/app/tests.rs`.
- [x] **Strip quoted diffs / residual bot scaffolding from comment bodies
      (token polish — low priority).** `strip_bot_boilerplate`
      (`src/app/pr_review.rs`) already removed `<details>`, HTML comments,
      `<summary>` tags, and images; it now also drops **fenced quoted-diff
      blocks** (```` ```diff ```` / ```` ```suggestion ````, matched
      line-anchored so an ordinary fenced code block like ` ```rust ` is left
      alone) and **leading `> ` blockquote lines** — the two ways CodeRabbit
      and similar bots re-paste a diff inline. Those repeated context the
      agent already gets for free from the comment's own `diff_hunk` plus the
      checked-out repo, inflating `fix_prompt()` for no value (principle #5).
      Only wired into the two call sites that already gate on `is_bot`
      (`agent_text()` and the list-snippet builder `make_snippet`), so the
      *displayed* detail-pane body (`PrComment::body`, unstripped) is
      untouched — a human's quoted diff still renders. Unit-tested (fence
      stripped, suggestion-fence stripped, leading `> ` lines stripped, a
      non-diff fence left byte-for-byte intact). → `src/app/pr_review.rs`.
- [ ] **AI attribution on AMF-posted comments (honesty — from real use).**
      When AMF posts content the agent harness generated (Epic E AI-review
      findings, and any future AI-drafted reply), append a **subtle, machine
      attribution footer** so reviewers can tell it was written by the agent
      harness *through AMF* — e.g. a one-line footer naming the harness
      (`— drafted by <harness> via AMF`) and/or a hidden marker for tooling.
      Scope it to **AI-authored** bodies: a user-typed reply (the Epic C
      "not-needed" reason, hand-edited templates) is the user's own words and
      shouldn't be misattributed — though an optional lighter "posted via
      AMF" tag for those is worth deciding (see open question). Make the exact
      footer text a single shared helper so replies and review posts stay
      consistent. → `src/app/pr_review.rs`, `src/github.rs`.
- [x] **BUG: triage/reply state is lost on return (from real use).**
      Root cause was the **head-SHA in the triage key**, not a missing flush:
      `pr_review_set_triage` (`m`/`s`), the reply post path, and the
      `f`-marks-`Fixing` path *all* already persist immediately, and
      `apply_persisted_triage` already runs on both the cache-hit and
      background-fetch loads. But `pr_comment_triage` was keyed by
      `PR# + comment_id + head_sha`, and the table doc even said it "starts fresh
      after a push moves the PR's head" — so the moment the agent's fix pushed a
      commit, returning via `G` re-resolved the PR to a **new** head SHA and the
      overlay looked up rows under a SHA that no longer matched, silently
      dropping every mark. A GitHub comment id is stable across commits, so
      triage is now keyed by `PR# + comment_id` and **survives a push**;
      `head_sha` is kept only as an informational record of the last SHA a mark
      was set under (migration **010** rebuilds the table with the new primary
      key, collapsing any per-SHA duplicates to the most-recently-updated row,
      and `load` drops the SHA filter). Unit-tested: the SHA-change survival at
      the DB layer, and the 009→010 migration collapse + new-PK enforcement. →
      `src/db/migrations.rs`, `src/db/pr_comment_triage.rs`, `src/db/mod.rs`,
      `src/app/pr_review.rs`.
- **Acceptance:** a 30-comment bot-heavy PR can be triaged quickly with
  measurably lower token spend than copy-paste round-trips.

### Epic E — AI code review & a learned review-findings memory

Everything up to here triages comments *other people* (and bots) already
left. This epic adds two coupled capabilities: **AMF generates its own
review of the diff**, and it **remembers what review keeps catching** so
each review starts smarter than the last. These are the first parts of
the feature that knowingly spend agent tokens on *generation* (not just
fix injection), so they're explicit, opt-in actions with a token preview
— and the memory doc is the lever that keeps that spend falling over
time: the more the team's recurring findings are written down, the less
the agent has to rediscover them from scratch on each review.

The two pieces form a loop: the **review-findings memory** is fed *into*
the AI reviewer as context (so it checks for the team's known issues
first), and the reviewer's output (plus comments triaged in the pane)
**feeds back into** the memory.

- [x] **Review-findings memory doc (committed markdown).** A
      version-controlled file at a conventional repo path (default
      `.amf/review-memory.md`, configurable) that accumulates the team's
      recurring code-review findings, grouped by category (concurrency,
      error handling, naming, tests, …). It lives in the repo so it's
      shared, diffable, and hand-editable, and so the AI reviewer can read
      it directly as context. AMF owns *appends* (dedup-aware, grouped) but
      never silently rewrites user prose. `review_memory_path(repo,
      configured)` resolves the doc path — a new `AppConfig::review_memory_path:
      Option<String>` overrides the default, relative overrides resolve
      against the repo root, absolute ones are used as-is.
      `ensure_review_memory_doc` creates the file (and parent dirs) with a
      header template on first write and is a no-op once it exists.
      `append_finding(path, category, finding)` creates the `## Category`
      section if missing (title-cased, blank category → `## General`), else
      inserts before the next `## ` heading so findings land in their
      section without bleeding into the next one; it's dedup-aware
      (case/whitespace-insensitive match against every existing line skips
      the append) and leaves any hand-written prose in the doc untouched.
      This is the primitive the next three Epic E items (lookback
      bootstrap, the pane's "add to memory" key, the AI reviewer's context
      injection) build on — no UI wiring yet. Unit-tested (path resolution
      incl. overrides, doc creation/no-op, section creation/reuse, section
      isolation, dedup, blank input, prose preservation). →
      `src/app/review_memory.rs`, `src/app/mod.rs`.
- [x] **Lookback bootstrap — distill the memory from the last *N* PRs.**
      Re-runnable action that seeds the memory doc from history instead of
      building it up one comment at a time. `b` in the PR picker
      (`AppMode::PrPicker`) opens a depth picker overlaid on the picker itself
      (`BootstrapPickState`, mirroring how the fix harness picker overlays the
      review pane) — **20 / 50 / 100 / all** recent PRs, 50 highlighted by
      default, with the resolved `review-memory.md` path shown. `⏎` resolves
      the merged/closed PRs synchronously via the new
      `GhCli::list_recent_closed_prs` (`--state closed`, which GitHub already
      folds merged into — confirmed live against this repo's own PRs, no
      client-side filtering needed) — cheap, so it's fine on the UI thread —
      then hands the heavy work to a background thread
      (`run_review_memory_bootstrap`) and switches to a full-screen running
      view (`AppMode::ReviewMemoryBootstrapRunning`, mirroring
      `PrReviewLoading`). The thread fetches each PR's review comments +
      summaries via the existing `GhCli::pr_review_comments`/`pr_reviews`
      (zero agent tokens; a single PR's fetch failure is skipped, not fatal),
      strips bot boilerplate the same way as everywhere else in this module,
      then makes **one** `ClaudeLauncher::run_headless` pass over one big
      assembled prompt (`bootstrap_prompt`) instructing the agent to cluster
      recurring findings into the same `## Category` / `- bullet` shape
      `append_finding` itself writes — so its response round-trips straight
      through a new `review_memory::parse_findings_markdown` with no further
      parsing, and reuses `append_finding`'s existing dedup (re-running over
      overlapping history is a no-op). Progress lands back over a channel
      (`BootstrapProgress`, polled each tick like the Epic A fetch): the
      running view shows "Fetching comments from N..." during the free `gh`
      loop, then "Distilling findings from N PRs (~N tokens)..." for the one
      paid pass, computed from the actual assembled prompt rather than a
      pre-run guess (the token count isn't knowable until the free fetch
      finishes, so the estimate is shown on the running screen rather than a
      separate pre-run confirm dialog as originally sketched). `esc` returns
      to the picker without waiting — the background run isn't aborted, and
      `poll_review_memory_bootstrap_bg` still surfaces its result (toast or
      error) whenever it lands, even if the user already navigated elsewhere,
      since it has a real side effect (tokens spent, findings written).
      Unit-tested (depth default/limits, prompt/text assembly, the pick
      dialog's open/move/cancel, and the poll function's stage updates +
      both the success and error return-to-picker paths). Confirmed live in
      an isolated tmux server against this repo's real merged PRs — caught
      and fixed two bugs in the process: a doubled "PRs PRs" in the fetching
      message, and `show_error` unconditionally resetting `self.mode` to
      `Normal` before the error path's restore-to-picker logic ran, which
      silently dropped the user onto the bare dashboard instead of the picker
      (and swallowed the error, since the picker's full-screen render doesn't
      draw `self.message`/toasts) — the fix captures the origin before any
      mode-mutating side effect and additionally writes the failure onto the
      restored picker's own inline `error` field. → `src/github.rs`,
      `src/app/review_memory.rs`, `src/app/pr_review.rs`, `src/app/state.rs`,
      `src/app/mod.rs`, `src/handlers/pr_review.rs`, `src/handlers/mod.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/ui/dialogs/mod.rs`,
      `src/ui/dashboard.rs`, `src/ui/status.rs`, `src/ui/dialogs/help.rs`,
      `src/main.rs`, `src/app/tests.rs`.
- [x] **"Add this comment to memory" key in the review pane.** `M` on the
      selected comment opens a confirm/edit dialog (mirrors the reply dialog's
      edit/confirm split) seeded from the new `PrComment::memory_finding_seed`
      — the bot-stripped `agent_text()` plus a `file:line` (or bare `file` for
      file-level comments) hint in parens, so a finding phrased as a general
      rule still carries where it came from. `Tab` in the confirm view cycles
      a category through a fixed list (`MEMORY_CATEGORIES`: General,
      Concurrency, Error handling, Naming, Tests, Performance, API design,
      Style — matching the doc's own header examples); `⏎` appends via
      `review_memory::append_finding` (whitespace/newlines collapsed to one
      line first, since the doc stores each finding as a single bullet),
      dedup-aware and append-only. The doc path resolves through the existing
      `repo_for_project_path` → `review_memory_path` (config override) chain,
      zero agent tokens — a local file write only, gated on the user's
      explicit confirm. This is the incremental, zero-extra-fetch path that
      grows the doc during normal review (complements the bulk lookback
      bootstrap, not yet built). Unit-tested (seed format + default category,
      edit/cancel key forwarding, category cycling wraps, empty-finding
      rejection with no doc write, append + dedup on re-add). →
      `src/app/state.rs` (`MemoryAddState`), `src/app/pr_review.rs`
      (`MEMORY_CATEGORIES`, `memory_finding_seed`), `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/ui/dialogs/help.rs`,
      `src/app/tests.rs`.
- [x] **Perform an AI code review of the PR (local draft → optionally post).**
      `A` in the review pane asks the agent to review the PR **diff**
      (`GhCli::pr_diff` → `gh pr diff <n>`, fetched in Rust) and surface
      findings. **Default: local draft** — the diff fetch + one headless
      `ClaudeLauncher::run_headless` pass run on a background thread (the same
      channel/poll/full-screen-running-view shape as the review-memory
      lookback bootstrap: `AiReviewProgress`/`AiReviewStage`,
      `AppMode::AiPrReviewRunning`, `App::poll_ai_pr_review_bg` wired into
      `main.rs`), with a token estimate shown once the prompt is assembled.
      The agent is instructed to emit a fixed `### path:line` / body shape
      (`ai_review_prompt`), parsed deterministically (`parse_ai_findings` —
      malformed or pathless headings degrade to a general/`### General`
      finding rather than erroring) and turned into ordinary draft
      `PrComment`s (`findings_to_comments`, synthetic ids from a high
      `AI_FINDING_ID_BASE` so they can never collide with a real GitHub
      comment id; a new `PrComment::ai_generated` flag, `#[serde(default)]`
      for cache backward-compat) merged into the pane's existing
      `review.comments` — so the whole existing list/detail/sort/filter/triage
      machinery (`f` inject-fix / `s` skip / `M` add-to-memory) works
      unchanged, with an `[AI]` chip in the detail header for visual
      distinction. `R`/`n` reply is guarded with a clear message on an
      unposted draft (no real GitHub thread to reply into yet); `x` resolve
      already degrades gracefully (no `thread_id`). The **review-findings
      memory is injected as context** (`review_memory_path` → read the file,
      empty string on miss) so the agent checks the team's known issues
      first — and a new `AppConfig::ai_review_skill: Option<String>` (e.g.
      `"review"`) optionally leads the prompt with an existing installed
      Claude Code review skill/command as the primary methodology, since AMF
      ships no bundled skill for reviewing a PR diff itself (checked: the
      repo's own `ai-review.md` command reviews AMF's *tracked-change
      history*, not a PR diff — unrelated); AMF still owns parsing the
      findings back out either way. **Persistence:** draft findings persist
      in the existing `pr_review_cache` JSON blob (no new table — `PrReview`/
      `PrComment` were already `Serialize`/`Deserialize`), so a same-head-SHA
      re-open replays them without re-spending tokens; a manual refresh (`r`)
      carries forward same-SHA drafts (`App::carry_forward_ai_drafts`, called
      before the fresh fetch overwrites the cache row) and correctly drops
      them at a new SHA (the drafts reviewed code that's since changed —
      re-run `A`). Each `A` run replaces the prior AI-draft set rather than
      accumulating duplicates. **Optional, explicit action:** `W` gathers
      every draft finding that isn't skipped or already posted, splits it into
      inline `GhCli::create_review` comments (anchored `path`+`line` findings)
      vs. a bulleted, editable summary (pathless/file-level findings —
      GitHub has no line-less inline comment), and posts as a `COMMENT`-event
      review (never auto-approve/request-changes, so no self-review 422 to
      handle) after an approve/edit confirm dialog (mirrors the reply dialog:
      edit the summary, `⏎` posts). On success every included finding is
      marked `Replied` (so a second `W` doesn't duplicate); on failure — e.g.
      GitHub's whole-review-rejected-if-any-comment-is-outside-the-diff
      contract — the drafts are left untouched, reusing `create_review`'s
      existing first-write `repo`-scope 403 handling. Unit-tested (prompt
      assembly incl. the skill directive, findings parsing incl. malformed/
      empty input, synthetic-id/kind assignment, the inline-vs-summary split,
      the background poll's progress/success/error/cancel paths, and the
      refresh-merge preserving same-SHA drafts while dropping them at a new
      SHA). → `src/app/pr_review.rs`, `src/app/state.rs`, `src/app/mod.rs`,
      `src/github.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/ui/dialogs/help.rs`, `src/main.rs`,
      `src/app/tests.rs`, `CHANGELOG.md`.
- **Acceptance:** bootstrap a `review-memory.md` from the last 50 PRs in
  one pass; run an AI review of an open PR that flags issues informed by
  that memory, triage its findings in-pane, optionally post them as a
  GitHub review; and, while reviewing, add a noteworthy comment to the
  memory with one key — each recurring finding written down once making
  the next review cheaper and sharper.

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

- **Highlight the logged-in user's own PRs in the PR picker.** `GhCli::list_prs`
  already returns each PR's `author` login (`src/github.rs`,
  `PrListEntry::author`), and `gh api user`/`gh auth status` can resolve the
  current account cheaply and cache it for the session (mirrors the existing
  `gh auth status` caching noted under the preconditions section). Use that to
  visually distinguish rows authored by the logged-in user in
  `draw_pr_picker` (`src/ui/dialogs/pr_review.rs`) — e.g. a distinct color,
  a `(you)` suffix, or sorting/grouping them first — so triaging your own
  open PRs (the common case: fix review comments left on work you authored)
  doesn't require reading every `@author` in the list to find them. Cheap and
  read-only; no new `gh` calls beyond what's already cached.

- **Rename the feature to "PR Triage."** "PR Comment Review" / "PR review"
  reads as passive (just reading comments) when the feature actually drives
  the whole loop — triaging, fixing, replying, resolving. "PR Triage" matches
  what the pane does and disambiguates from the separate Epic E "AI code
  review" capability once that ships (a real *review* of the diff, vs.
  *triaging* what reviewers already said). Touches user-facing strings across
  the dashboard help entry (`G` — "Review PR comments"), the pane title/header,
  the manual PR-number/PR-picker prompts, status-bar/loading labels, and the
  keybindings help sections — plus, more carefully, the internal
  `REVIEW_SESSION_LABEL = "PR Review"` constant (`src/github.rs`/
  `src/app/pr_review.rs`) that names the dedicated tmux session:
  renaming that label changes what `fix_session_index` matches against, so
  existing dedicated review sessions created under the old label would need a
  migration note (or the lookup would need to accept both labels for a
  transition period) rather than silently losing track of already-running
  sessions. Pure rename — no behavior change otherwise.

- **Dedicated review-session status badge in the PR review pane.** The
  Viewing-mode corner badge (`[Ctrl+Space P: back to review]`,
  `src/ui/dashboard.rs`) tells you when you're *inside* the dedicated
  review session, but there's no equivalent the other way round: sitting in
  the review pane while `f` has a fix running in the background, nothing
  in-pane shows whether that dedicated session even exists yet, or whether
  it's actively working vs. idle/finished. Add a small header badge —
  alongside the Epic D "token usage surfaced per session" span already in
  `draw_pr_review`'s header (`src/ui/dialogs/pr_review.rs`) — that reads
  something like `[dedicated ● working]` / `[dedicated idle]` once
  `fix_session_index` resolves a session, reusing the same
  `thinking_features` tracking `App::is_feature_thinking` already exposes
  (`src/app/sync.rs`). Gotcha to design around: `is_feature_thinking` is
  keyed by `tmux_session` at the *feature* level, not per-window, so as-is
  it can't distinguish "the dedicated review session is thinking" from
  "some other window in this feature is thinking" when they share a tmux
  session — likely needs a per-window/per-session variant of the thinking
  probe, or a pane-content check scoped to the review session's window
  specifically.

## Open questions

- **Which agent session runs the fixes? — DECIDED.** Default to spinning
  up (and reusing) one **dedicated review session** for all of the PR's
  fixes; offer **reuse-the-existing-live-session** as an option; never
  one session per comment. Rationale and the options considered:
  - **Reuse the existing feature session (current plan).** Warm: the
    agent already has codebase context, and the cached prompt prefix
    (Anthropic prompt cache, ~5-min TTL) makes continuing cheap. But the
    session may already carry a long, unrelated conversation, so every
    fix turn pays for that bloated context, and review work can pollute /
    disrupt the user's in-progress work.
  - **One dedicated review session for *all* fixes.** A single fresh
    harness session used only for this PR's fixes, sequentially. Pays the
    fixed per-session overhead (system prompt, tool definitions, any
    skill injection) **once**, and amortizes file reads across comments —
    especially valuable when several comments touch the same file. Grows
    over a long PR, but compaction/caching handle that.
  - **A new session per fix.** Cleanest isolation, smallest context per
    fix — but **worst for tokens**: it repeats the fixed per-session
    overhead N times and re-reads the same files cold for every comment.
  - **Decision:** the **dedicated review session is the default** — the
    pane spins one up the first time the user fixes a comment and reuses
    it for the rest of the PR (overhead paid once, file context reused,
    cache-friendly, and it keeps review work out of the user's working
    session). **Reusing the feature's existing live session is an opt-in
    choice** for when warm in-progress context is wanted. One-per-fix is
    rejected. Relates to token principles #4 (one persistent session) and
    #8 (opt-in batch). _Still open:_ exactly how the user toggles the
    choice (per-PR setting vs. a key in the pane vs. config default), and
    how the dedicated session's lifecycle/cleanup is surfaced.
- **Reply identity — DECIDED (attribute AI-authored content).** Replies
  post as the user's `gh` auth. Real use confirmed we **do** want a subtle
  footer attributing **AI-authored** content to the agent harness via AMF
  (now the Epic D "AI attribution" item). _Still open:_ whether
  *user-typed* content posted through AMF (the not-needed reason, a
  hand-edited template) should also carry a lighter "posted via AMF" tag,
  or stay unmarked as genuinely the user's words.
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
- **Review-memory path & scope (Epic E):** default `.amf/review-memory.md`
  committed in the repo — but should the path be configurable per project,
  and should there be an optional *global* layer (cross-project lessons)
  merged in on top of the per-repo file?
- **Memory growth without rot (Epic E):** appends dedup against existing
  entries, but the doc will still drift/bloat. Periodic agent "compaction"
  pass to merge near-duplicates and prune stale rules, or leave curation
  fully manual?
- **AI-review model & cost (Epic E):** the diff review and the lookback
  distill are the token-heavy steps. Same harness as fixes, or a
  dedicated review model? And how big a diff do we send before chunking /
  refusing (context-window ceiling, like the batch-prompt cap)?
- **Posting AI-review findings (Epic E):** when posting as a real GitHub
  review, tag AMF-generated comments for honesty (subtle footer), same
  open question as AI-drafted replies above?

## Reasoning / when to build

Build after the prompt-library injection seam is stable (Epic B depends
on it). Epic A is independently valuable and low-risk (read-only, no
agent, no writes) — a good first slice to ship and validate the fetch /
normalize / strip pipeline before wiring in any agent or GitHub writes.
The token discipline (metadata-first, minimal fix prompts, warm-session
reuse, caching) is what makes high-volume review loops affordable and is
the reason to do this in AMF rather than in the agent chat directly.
