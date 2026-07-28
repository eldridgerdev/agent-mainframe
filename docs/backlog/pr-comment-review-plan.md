# PR Triage

- **Status:** Shipped — all epics and open questions closed; one small
  backlog item remains (compacting the global review-memory doc)
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
- [x] **Tag AMF-templated (non-AI) replies with a "posted via AMF" footer
      (resolved open question — channel disclosure, not authorship).** The
      AI-attribution footer (Epic D) only covers genuinely AI-*generated*
      content (Epic E findings); the "Done in `<sha>`" and "not-needed"
      templates are the user's own words and were posting with no marker at
      all. Decided: mark the *channel* rather than authorship — a reader on
      GitHub should be able to tell a reply came through tooling even though
      the words are the user's. A new `append_amf_attribution` (distinct
      wording from `append_ai_attribution`, `src/app/pr_review.rs`) appends
      `— posted via AMF` to the body `pr_review_post_reply` sends to GitHub;
      the locally-persisted `local_note` for a "not needed" reply stays the
      unmarked text (it's AMF's own record, not something read on GitHub).
      Applied at post time rather than folded into the editable seed — an
      empty "not needed" buffer starting with a footer already in it would
      be awkward to type into — so the reply dialog gained a
      token-preview-style disclosure line ("will post with a `— posted via
      AMF` footer") between the editor and the key hints instead. Unit-tested
      (`append_amf_attribution` wording/trimming; a render test proving the
      disclosure line appears in the dialog). → `src/app/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`.
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
- [~] **Queue marked comments as separate prompts — removed.** This earlier
      throughput loop was superseded by the safer combined-prompt workflow
      below and removed in the later keymap-simplification backlog item.
      Previously, `space` toggled a **batch mark** on the selected
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
      launch-and-leave: **every included comment is marked `Fixing` and
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
- [x] **Group conversation comments into their own section (resolved open
      question).** Conversation comments (no `path`/resolution) interleaved
      with inline/review comments in every sort order, easy to lose in a
      busy PR. A fifth `PrSortMode::Conversations` (cycled with `o`, after
      `HumansFirst`) groups them after every code-anchored comment — stable,
      so relative order within each group is unchanged — via the same
      `sort_indices` mechanism the other modes use. Went with "a real
      section," not just a silent reorder: a new
      `PrReviewState::conversation_section_start()` reports where the
      visible list's conversation group begins (`None` outside this mode, or
      when the visible set has no conversation comments, or is *entirely*
      conversation comments — nothing to separate either way), and
      `draw_comment_list` inserts a "─ Conversation ─" divider row there,
      shifting the highlight-index lookup by one past the divider. Footer/
      keybinding-help updated to list the new mode. Unit-tested (cycling now
      wraps through five modes; an already-grouped fetch order is a no-op;
      an interleaved one reorders into two stable groups;
      `conversation_section_start` in/outside the mode and with/without both
      groups present; a render test for the divider showing only under this
      mode). → `src/app/pr_review.rs`, `src/app/state.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/ui/dialogs/help.rs`,
      `src/app/tests.rs`.
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
- [x] **AI attribution on AMF-posted comments (honesty — from real use).**
      A new shared `append_ai_attribution` helper (`src/app/pr_review.rs`)
      appends `— drafted by Claude via AMF` to AI-authored GitHub content —
      wired into `build_ai_review`'s inline-comment bodies (Epic E `W` — "post
      as GitHub review"), which previously posted `f.body` verbatim with no
      attribution of their own. The top-level review summary already
      self-identifies (`"AI review, via AMF."` as its opening line, unchanged),
      but each *inline* comment can surface on its own — e.g. GitHub's
      Files-changed view — without the summary in sight, so it needed its own
      marker. Hardcoded to "Claude" rather than a generic `<harness>` slot:
      AI-review generation always runs through
      `ClaudeLauncher::run_headless`, independent of whichever harness a
      "fix" gets injected into (`review_harness` only targets the fix
      session), so there's no other harness it could currently be. Scoped to
      AI-authored bodies only, per the original design — a user-typed reply
      (the Epic C "not-needed" reason, hand-edited "done in `<sha>`"
      template) is the user's own words and stays unmarked; the general/
      pathless findings folded into the summary bullet list aren't
      individually footed since the summary's opening line already covers
      them. One shared helper so any future AI-drafted reply routes through
      the same wording. Unit-tested (`append_ai_attribution` appends the
      footer and trims trailing whitespace first; `build_ai_review`'s inline
      body now includes it). → `src/app/pr_review.rs`.
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

      **Follow-up from real use, same day.** `esc`-ing back to the pane while
      the background pass was still running left no trace that anything was
      happening — the running screen's throbber disappears with it, and
      nothing in the ordinary pane hinted a review was in flight. The header
      now shows a throbber + "AI review running…" for as long as
      `App::ai_review_bg` is `Some`, and `App::has_visible_animation` gained
      a `PrReview` arm keyed off the same field so the throbber actually
      advances frame-to-frame while sitting in the pane (previously
      `PrReview` fell through to the default `false`, so nothing forced a
      redraw cadence). Unit-tested (`has_visible_animation` toggles with
      `ai_review_bg`). → `src/ui/dialogs/pr_review.rs`, `src/ui/dashboard.rs`,
      `src/app/mod.rs`, `src/app/tests.rs`.

      **Second follow-up, same day.** The running screen itself (before
      backing out) had the identical bug: `redraw_signature()` only hashes
      the mode's *discriminant*, so a stage change within
      `AppMode::AiPrReviewRunning` doesn't register as "state changed," and
      the throbber only advanced on the rare frame the background poll
      itself forced a redraw — otherwise it sat visibly frozen for the whole
      blocking `gh pr diff` / `claude -p` call, reading as stuck. Same root
      cause on two sibling screens that share the pattern
      (`AppMode::PrReviewLoading`, `AppMode::ReviewMemoryBootstrapRunning`),
      so all three gained an unconditional `has_visible_animation` arm
      (mirroring how `DiffViewerLoading`/`MarkdownLoading` are already
      handled) rather than just the one that was reported. Unit-tested
      (all three modes report `has_visible_animation() == true`). →
      `src/app/mod.rs`, `src/app/tests.rs`.

      **Third follow-up, same day.** With the throbber fixed, the actual
      report was "the review runs, says it's done, but nothing happens" —
      two compounding bugs. (1) `PrReviewLoading`/`PrReview`/`PrPicker`/
      `ReviewMemoryBootstrapRunning`/`AiPrReviewRunning` each `return` from
      `ui::dashboard::draw` before reaching the shared `draw_toasts` call, so
      *every* toast pushed while any of them is showing — including the AI
      review's own "found N findings" result — was silently swallowed; all
      five now draw toasts too. (2) `parse_ai_findings` required the exact
      `###` heading level `ai_review_prompt` asks for; a model that
      substitutes `##`/`#` (common) or wraps its whole reply in a code fence
      (also common, despite "no prose outside it") now parses to **zero**
      findings with nothing to show and no error — indistinguishable from a
      genuinely clean diff. The parser now accepts any 1-4-level heading
      (`strip_finding_heading`) and strips a single outer code fence
      (`strip_outer_code_fence`) before parsing; `AiReviewProgress::Done`'s
      success payload became `AiReviewOutcome { findings, raw_output }` so
      [`App::poll_ai_pr_review_bg`] can tell the two zero-finding cases apart
      — a **0-finding** result now pushes a *warning* toast pointing at the
      debug log (`D`) instead of a quiet success, and the raw response is
      always logged (`log_warn` when 0 findings from non-empty output,
      `log_debug` otherwise) so a persistent mismatch is diagnosable without
      re-running anything. Unit-tested (both new parse helpers; the poll's
      zero-vs-nonzero toast/log branches indirectly via existing coverage). →
      `src/app/pr_review.rs`, `src/ui/dashboard.rs`, `src/app/tests.rs`.

      **Fourth follow-up, same day — the actual root cause.** Neither of the
      above was it. Real-world use hit `claude headless command failed`
      directly (not a silent 0-finding parse miss). Reproduced outside AMF:
      `ClaudeLauncher::run_headless`/`spawn_headless` pass the prompt as a
      `-p <prompt>` `argv` element, and Linux caps any single argument at
      `MAX_ARG_STRLEN` (128 KiB, independent of the ~2 MiB total `ARG_MAX`) —
      confirmed via a standalone Rust repro: a ~190 KB prompt makes
      `Command::output()` return `Err(ArgumentListTooLong)` (`E2BIG`)
      *before* `claude` ever runs. A real PR diff clears 128 KB routinely.
      Piping the same prompt over stdin instead (`cat prompt | claude -p
      --output-format text`) was verified working up to 300+ KB against the
      live binary, including a case where the model correctly flagged a bug
      in the padding content — so both `run_headless` (blocking, via a
      writer thread + `wait_with_output` to avoid a pipe deadlock between
      writing stdin and draining stdout/stderr) and `spawn_headless`
      (non-blocking, via a detached writer thread so the call keeps returning
      the `Child` immediately) now write the prompt to piped stdin instead of
      `argv`. This is shared infrastructure, not specific to this pane: it
      also fixes the review-memory lookback bootstrap and final review's
      diff walkthrough / co-review / changeset overview for any prompt over
      the same limit. A live-`claude` regression test lives at
      `claude::tests::run_headless_handles_a_prompt_over_the_argv_limit`,
      `#[ignore]`d (matches this file's existing no-live-external-calls
      convention) — run manually with `cargo test -- --ignored
      run_headless_handles_a_prompt_over_the_argv_limit`. →
      `src/claude.rs`.

      **Fifth follow-up, same day.** With the review actually completing,
      real use against this PR's own live diff surfaced three more gaps —
      good evidence the "does this belong in this pane" question above is
      the right one to sit with. (1) `findings_to_comments` builds a draft
      with `is_bot: false`, and the role chip fell through straight to
      `[human]` — a synthetic finding is neither; it now checks
      `ai_generated` first and shows `[ai]` (the marker-legend footer text
      updated to match). (2) A draft finding never had a `diff_hunk` at all
      — nothing about *generating* a finding produces one the way GitHub's
      API hands one over for free on a *fetched* comment — so the detail
      pane's whole "Diff hunk" section was silently absent for every AI
      finding. Fixed by reusing the diff parser already built for the
      Final Review feature (`crate::diff::parse_unified_diff`, `DiffFile`/
      `DiffHunk`): `run_ai_pr_review` parses the already-fetched PR diff once
      and re-matches each anchored finding's `path:line` back into it
      (`diff_hunk_for_line`), reconstructing the same `@@ ... @@`-plus-body
      shape GitHub returns so it renders and injects exactly like a fetched
      comment's hunk would (`None` when the line doesn't land in any hunk —
      degrades the same way an unavailable GitHub hunk already does).
      (3) The real bug from the "does this belong here" analysis above:
      `esc` (`cancel_ai_pr_review`) restores `self.mode` to `PrReview` well
      before the background pass finishes; `Done` used to look for
      `AppMode::AiPrReviewRunning` to find where to merge, and once
      cancelled that match failed — findings were parsed, logged, and then
      never merged into any comment list, while the success/warning toast
      still fired unconditionally, falsely announcing results that were
      silently thrown away. Fixed with a new `App::ai_review_pending:
      Option<PrReviewState>`, set alongside `ai_review_bg` in
      `start_ai_pr_review` and *not* cleared by cancel, so `Done`'s handler
      can always find where the review was for. It now resolves a `Target`
      (`Running` / `Pane` / `Elsewhere`) from the *current* `self.mode`
      before merging — `Pane` merges into whatever live state the user has
      (including further triage made after cancelling, not the stale
      pre-`A` snapshot), `Elsewhere` (navigated to a different PR/pane
      entirely) caches without touching `self.mode`, and the toast notes
      "(re-open to see it)" for that case. `start_ai_pr_review` also gained
      a re-entrancy guard (a second `A` while one is already running now
      warns instead of orphaning the first). Unit-tested (the exact
      cancel-then-triage-then-Done sequence, the re-entrancy guard, plus
      `diff_hunk_for_line` against a real parsed diff). →
      `src/app/pr_review.rs`, `src/app/mod.rs`, `src/ui/dialogs/pr_review.rs`,
      `src/app/tests.rs`.

      **Sixth follow-up, same day.** Item (2) above still showed "the whole
      file" — `diff_hunk_for_line` was reconstructing the *entire* matched
      hunk verbatim, and this PR's own diff has hunks that are large
      contiguous blocks of new code (whole functions added in one place),
      so "the matching hunk" and "the whole file region" were
      indistinguishable in practice. Rewrote it to walk the hunk tracking
      old/new line numbers, locate the target line's index, and slice a
      fixed `AI_FINDING_HUNK_CONTEXT_LINES` (6) window of lines on each
      side — capped regardless of how large the real hunk is — then
      synthesizes a matching `@@ ... @@` header for the trimmed range
      rather than reusing the original hunk's. Unit-tested against a
      40-line synthetic added-lines hunk: the returned window is bounded,
      contains the target line, and excludes lines far to either side. →
      `src/app/pr_review.rs`.

      **Seventh follow-up, same day.** GitHub-fetched comments still bypassed
      that windowing: in real use, an outdated single-line comment arrived
      with a 92-line `diff_hunk`, so the detail pane and fix prompt buried the
      referenced line in an entire newly-added function. `PrComment::prompt_hunk`
      now parses any large line-anchored GitHub hunk and rebuilds it around the
      comment's actual old/new-side line with three surrounding lines on each
      side and a corrected `@@` header. The transform happens when the hunk is
      consumed, so existing cached reviews improve without a refresh; file-level
      comments and malformed unanchored oversized hunks keep the existing
      suppression safety net. Unit-tested against a 40-line added block, with
      the target retained and distant lines excluded. → `src/app/pr_review.rs`,
      `src/github.rs`, `src/ui/dialogs/pr_review.rs`.

      **Eighth follow-up — surface `A` failures as toasts (from real use).** A
      failed background AI-review run called the shared `show_error`, which
      stored the failure in AMF's dashboard status message. The result handler
      then restored the full-screen PR Triage pane, where that message is not
      rendered, so the user saw the run end without being able to read the
      error. AI-review worker failures now remain logged with their full detail
      and also push an eight-second error toast before returning to the pane;
      unexpected worker-channel termination uses the same visible path. The
      normal pane state is preserved, including when the user already cancelled
      the running screen while the job continued in the background. Focused
      tests cover both a returned worker error and a disconnected worker. →
      `src/app/pr_review.rs`, `src/app/tests.rs`.
- [x] **Choose the agent harness for `A` AI reviews.** The first `A` in a PR
      Triage pane now opens an independent single-select picker over the
      project's allowed agents, highlights the project's preferred harness,
      and remembers the choice in `PrReviewState::ai_review_harness` for later
      `A` runs in that pane. This is deliberately separate from
      `review_harness`, so one provider can generate review findings while
      another owns the dedicated `f`/`B` fix session. Confirming validates the
      selected CLI before the diff fetch or paid pass starts; an unavailable
      provider leaves the picker open with its actionable error, while cancel
      returns to the pane with no background job or token spend. The footer and
      running screen name the selected review harness.

      A new typed `HeadlessRunner` routes the existing background lifecycle
      through all four built-in agents: Claude print mode, Codex `exec`,
      OpenCode `run`, and Pi print mode. Every runner receives the prompt over
      piped stdin (including OpenCode, whose current `run` implementation
      explicitly merges piped stdin into its message), preserving support for
      PR diffs beyond Linux's per-argument size limit; stdout is drained while
      a writer thread feeds stdin to avoid large-prompt/large-response pipe
      deadlocks. Spawn, write, exit-status, and stderr failures name the chosen
      provider and continue through the existing PR Triage error-toast path.
      Since posted findings are no longer necessarily Claude-authored, their
      attribution is now the accurate provider-neutral `drafted by AI via AMF`.
      Focused tests cover every runner command, stdin delivery,
      provider-specific stderr, preferred-default picker state, remembered
      choice, and cancellation before spend. → `src/headless.rs`,
      `src/app/state.rs`, `src/app/pr_review.rs`,
      `src/handlers/pr_review.rs`, `src/ui/dialogs/pr_review.rs`,
      `src/ui/dialogs/help.rs`, `src/app/tests.rs`.
- **Acceptance:** bootstrap a `review-memory.md` from the last 50 PRs in
  one pass; run an AI review of an open PR that flags issues informed by
  that memory, triage its findings in-pane, optionally post them as a
  GitHub review; and, while reviewing, add a noteworthy comment to the
  memory with one key — each recurring finding written down once making
  the next review cheaper and sharper.
 - [x] **BUG: `W` post failure gives a cryptic error and kicks you out of the
      pane (from real use).** Reported: pressing `W` to post the AI review
      showed `Error: `gh api` (create review) failed: gh: Unprocessable
      Entity (HTTP 422)` (confirmed via the debug log, `D`) and dropped
      straight back to the dashboard. **Two compounding problems, both
      fixed:** (1) The 422 itself is GitHub's documented behavior for
      `GhCli::create_review` — the whole review is rejected if *any* inline
      comment's `path`/`line` doesn't land inside the PR's current diff —
      but `gh api`'s stderr for it is just the terse `Unprocessable Entity
      (HTTP 422)` line with no indication of *which* finding is the culprit,
      and AMF was passing it straight through with no translation. Fixed
      with a new `is_review_rejected_entity_error` check (sibling to the
      existing `is_missing_write_scope`, `src/github.rs`) that detects the
      422/"Unprocessable Entity" case and bails with an actionable message
      instead (likely a stale AI finding since the diff moved — refresh and
      re-run `A`, or skip it and retry `W`). (2) Far worse: on *any* error,
      `pr_review_post_ai_review` called `self.show_error(e)`
      (`src/app/project_ops.rs`), which unconditionally sets
      `self.mode = AppMode::Normal` for every mode except `Normal`/`Help`/
      `Viewing` — `PrReview` wasn't exempted, so a recoverable posting
      failure silently booted the user all the way back to the dashboard.
      Fixed by extracting a new `App::fail_ai_review_post` helper: it
      captures the live `PrReviewState` (with the still-open post-confirm
      dialog) before calling `show_error`, records the failure on a new
      `AiReviewPostConfirmState::error` field, then restores `self.mode` to
      that captured pane afterward — so the dialog stays open with the
      error shown inline (`draw_ai_review_post_dialog`,
      `src/ui/dialogs/pr_review.rs`, a new `danger`-styled row above the
      summary body) instead of losing the pane. `show_error` itself is
      untouched (still the shared logging/toast/message path other modes
      rely on); only this one call site now works around its mode reset,
      matching the same "capture origin before show_error, restore after"
      pattern already used by `poll_ai_pr_review_bg` and
      `poll_review_memory_bootstrap_bg`. Unit-tested
      (`is_review_rejected_entity_error` against the exact real-use stderr
      line and other error strings; `fail_ai_review_post` keeps the dialog
      open with the error recorded rather than falling back to
      `AppMode::Normal` — `GhCli::create_review` itself isn't invoked in
      tests, matching this file's no-live-`gh`-calls convention). →
      `src/github.rs`, `src/app/pr_review.rs`, `src/app/state.rs`,
       `src/ui/dialogs/pr_review.rs`, `src/app/tests.rs`.

      **Follow-up, same day — the actual root cause.** The "stale finding"
      diagnosis above was wrong: real use hit the *identical* 422 immediately
      after `r` (refresh) + `A` (re-run the AI review) — a fresh diff and a
      fresh model pass reproducing the same failure rules out staleness.
      The real cause is that models compute `<path>:<line>` from the raw
      unified-diff text themselves (`ai_review_prompt` shows the diff, not
      pre-computed line numbers) by mentally counting from the `@@ -a,b +c,d
      @@` hunk headers — an inherently error-prone arithmetic task, and a
      miscount reproduces identically on every re-run since it's not a
      function of *when* the diff was fetched. AMF already had the exact
      signal for this: `run_ai_pr_review` matches each finding's `path:line`
      back into the fetched diff via `diff_hunk_for_line`, and a line that
      doesn't correspond to anything in the diff returns `None` — but
      `build_ai_review` wasn't consulting that signal before deciding a
      finding was inline-postable, so a finding with a miscounted line got
      sent to GitHub's create-review API anyway, which validates the exact
      same thing (a `line` outside the diff's hunks) and rejects the whole
      review. Fixed by adding `f.diff_hunk.is_some()` to `build_ai_review`'s
      inline-eligibility guard — a finding whose line didn't match anything
      in the diff (and isn't file-level) now folds into the summary bullet
      list (with its `path:line` kept as context, unlike the file-level
      case which drops the line it never had) instead of a doomed inline
      post, eliminating the whole failure class rather than just handling
      it better after the fact. Unit-tested (a finding with no matching hunk
      folds into the summary rather than the inline list; the two existing
      `build_ai_review` tests updated to set a matching `diff_hunk` on their
      still-inline-expected fixtures). → `src/app/pr_review.rs`,
      `src/app/tests.rs`.
- [x] **Make the review-memory path configurable per project (resolves half
      of the Open Questions item below).** `AppConfig::review_memory_path`
      was already a path override, but global-only — every project shared
      it. `ExtensionConfig` (`src/extension.rs`) gains a
      `review_memory_path: Option<String>` field following the exact
      "project overrides global" shape `final_review_check_command` already
      established: a project's `{repo}/.amf/config.json` value wins,
      falling back to `merge_project_extension_config`'s global `base`. A
      new `App::configured_review_memory_path(repo)` (`src/app/mod.rs`)
      resolves that project-scoped `ExtensionConfig` value first, then
      falls back to the pre-existing global `AppConfig::review_memory_path`
      — so an existing global setting keeps working unchanged for projects
      that don't opt into an override — before `review_memory::
      review_memory_path` applies its own `DEFAULT_REVIEW_MEMORY_PATH`
      fallback. All four call sites (AI-review context read, manual "add to
      memory", the lookback bootstrap, and the PR-picker's memory-path
      display) now route through it instead of reading
      `self.config.review_memory_path` directly. The config wizard has no
      UI for this field (same as `final_review_check_command`), so
      `build_extension_config` carries a loaded value through untouched
      rather than dropping it on save. Unit-tested (project-overrides-
      global and global-fallback merge cases in `extension.rs`; an
      app-level test confirming a project override actually redirects
      where `pr_review_append_memory` writes, leaving the default path
      untouched). → `src/extension.rs`, `src/app/mod.rs`,
      `src/app/pr_review.rs`, `src/app/config_wizard.rs`,
      `src/ui/dashboard.rs`, `src/app/tests.rs`.
- [x] **Prevent review-memory rot (resolves the Open Questions item of the
      same name).** Findings only ever accumulated (`M`, the lookback
      bootstrap) with no pruning, so the doc could drift/bloat with
      near-duplicate or stale rules over time. `c` in the PR picker opens a
      confirm overlay showing how many findings are in the doc today
      (`review_memory::count_findings`, a plain local read — free), then a
      single headless agent pass (always Claude, mirroring the lookback
      bootstrap's harness choice) rewrites the whole doc: merge
      near-duplicates, drop stale/superseded/overly-specific findings,
      preserve section structure and hand-written prose
      (`review_memory::compact_prompt`). Unlike every other review-memory
      write, this is **not** append-only — `review_memory.rs`'s header now
      documents the one explicit exception. So nothing touches disk until
      the user reviews the proposal: the background pass (`CompactProgress`,
      `run_review_memory_compact`) reports the full proposed replacement to
      a new full-screen, editable `AppMode::ReviewMemoryCompactReview`
      (mirrors `draw_fix_confirm`'s edit/scroll handling), and only `⏎`/`w`
      (`App::pr_review_compact_write`) writes it — `esc` discards, doc
      untouched. `esc` from the running screen (`AppMode::
      ReviewMemoryCompactRunning`) returns to the picker without aborting
      the background pass, same as the bootstrap/`A` running screens; a late
      result after that no longer has anywhere live to land a full-screen
      dialog without yanking the user out of whatever they're doing, so it
      toasts instead of reopening — tracked via a new `App::
      review_memory_compact_pending`, the same fix already shipped for `A`'s
      `ai_review_pending` after the "findings silently dropped after an
      `esc`" bug. Unit-tested (confirm-overlay open/bail-when-empty/cancel,
      poll success/empty/error transitions, the late-result-after-cancel
      toast-not-reopen path, write-overwrites-and-toasts, discard-leaves-
      file-untouched, plus `compact_prompt`/`count_findings` directly) — full
      suite green (1151 tests) and `cargo clippy` clean. →
      `src/app/review_memory.rs`, `src/app/pr_review.rs`,
      `src/app/state.rs`, `src/app/mod.rs`, `src/main.rs`,
      `src/handlers/pr_review.rs`, `src/handlers/mod.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/ui/dialogs/mod.rs`,
      `src/ui/dialogs/help.rs`, `src/ui/dashboard.rs`, `src/ui/status.rs`,
      `src/app/tests.rs`, `CHANGELOG.md`.
- [x] **Cross-project (global) review-memory layer (resolves the last Open
      Questions item).** Per-project *path* configurability had already
      shipped; what was undecided was *content* — whether a separate
      cross-project lessons file should be merged on top of each repo's doc,
      or per-repo stay strictly isolated. **Decided: merge one in.** Review
      lessons split cleanly into house rules (this repo's `.amf/review-memory.md`,
      committed and shared) and personal habits that follow you between repos,
      and there was no home for the second kind — so they were either retyped
      per repo or lost. AMF now keeps one at
      `~/.config/amf/review-memory.md`, resolved by a new
      `review_memory::global_review_memory_path` (relative overrides anchor to
      the config dir, absolute ones are used as-is, override key
      `AppConfig::global_review_memory_path` — global-only by nature, so
      unlike `review_memory_path` there's deliberately no `ExtensionConfig`
      per-project counterpart). A `MemoryScope` (`Project`/`Global`) enum now
      threads through the module: it picks the header template a
      newly-created doc gets (the global one says "cross-project" and explains
      the split) and is a required argument to `append_finding`, so no write
      can reach a doc without having named which one.

      **Reads merge; writes pick one.** Only the AI reviewer reads both, via a
      new `merge_memory_context(project, global)`: one doc alone comes back
      verbatim (so the pre-existing single-doc case is byte-identical), and
      when both have content each is introduced by a plain-text label — not a
      Markdown heading, which would collide with the `## Category` headings
      inside the docs themselves — with the project doc first. Global findings
      the project doc already states are pruned first (`prune_duplicate_findings`,
      case/whitespace-insensitive, also dropping any global section left with
      no bullets), because the two docs overlap *by design* — promoting a rule
      from one repo to all of them is the point — and paying for the same rule
      twice in every review prompt is exactly the token waste this feature
      exists to avoid. `ai_review_prompt`'s context header stopped saying "for
      this project" to match. Every write flow still targets exactly one doc,
      chosen by the user: `g` toggles the scope in the `M` "add to memory"
      dialog and in the bootstrap's (`b`) depth picker, both defaulting to
      `Project` (a finding from this PR, or distilled from this repo's PR
      history, is about this repo until the user says otherwise) and both
      naming the destination *file* on screen, so "global" is never a guess
      about where the finding went. Toasts and the bootstrap running screen
      name the scope too. Compacting (`c`) deliberately stays project-only —
      see the Backlog item below.

      The four scattered path lookups collapsed into one
      `App::review_memory_paths(repo) -> ReviewMemoryPaths` holding both, with
      `for_scope`. The pane's renderer resolves it **only while the `M` dialog
      is open** (`Option<&ReviewMemoryPaths>`): `repo_for_project_path` shells
      out to git, and `draw_pr_review` runs every frame. Unit-tested
      (global path resolution incl. both override shapes, scope-specific
      headers, a global-scope `append_finding` creating the cross-project doc,
      and five `merge_memory_context` cases: single-doc verbatim, labeled
      both-docs ordering, duplicate pruning, an emptied section dropped, and a
      wholly-duplicated global doc collapsing back to project-only), plus
      App-level tests (scope defaults + toggles for both dialogs, a global
      write leaving the project doc untouched, and combined project-override /
      global resolution) and render tests proving each dialog names the right
      file per scope. Full suite green (1331 passed, 1 ignored), `cargo
      clippy`/`cargo fmt` clean. → `src/app/review_memory.rs`, `src/app/mod.rs`,
      `src/app/pr_review.rs`, `src/app/ai_review.rs`, `src/app/state.rs`,
      `src/handlers/pr_review.rs`, `src/ui/dialogs/pr_review.rs`,
      `src/ui/dialogs/help.rs`, `src/ui/dashboard.rs`, `src/app/tests.rs`,
      `CHANGELOG.md`.

## Nice to have

- [x] **BUG — support sequential/multiple PRs for the same feature branch.**
      When a feature branch is reused for another PR, AMF can keep showing the
      previous closed PR instead of the current open one (observed here: the
      feature still reports closed PR #449 while work has moved to PR #450).
      Treat the PR number as changing feature state rather than a permanent
      branch identity: branch auto-detection should prefer the current open PR
      when GitHub has multiple PRs for the same head branch, and a transition
      to a different PR number must invalidate or replace the active-PR badge,
      cached `PrReviewState`, pending AI-review target, and `P` return stash
      associated with the old PR. Preserve each PR's own SQLite comment/cache
      history under its existing PR-number keys, but never let reopening the
      feature silently restore a closed predecessor. Manual selection of a
      closed PR in the picker must remain possible and explicit. Add regression
      coverage for closed `#N` + open `#N+1` on one branch, including dashboard
      badge refresh, `G` auto-entry, and returning from the linked triage
      session. Shipped by replacing bare `gh pr view` auto-detection with a
      branch-scoped all-state query that explicitly selects the newest open PR.
      A PR-number transition now replaces the active badge and invalidates old
      in-memory return/loading/AI-review targets without deleting either PR's
      SQLite cache or triage history; an explicitly selected closed PR can stay
      open, but can no longer be restored implicitly after its open successor is
      detected. Regression tests cover closed-to-open selection, multiple-open
      tie-breaking, badge replacement, stale target invalidation, and the linked
      triage-session return path. → `src/github.rs`, `src/app/pr_review.rs`,
      `src/app/sync.rs`, `src/app/tests.rs`.

- [x] **BUG — posted AI-review findings remain trapped as drafts after `W`.**
      After generating findings with `A` and successfully posting them as a
      GitHub review with `W`, the pane still treats each finding as an
      unposted synthetic AI draft. The user cannot use the normal follow-up
      workflow to mark it done and post a `Done in <sha>` reply; the action is
      rejected with the "AI draft" message even though the finding now exists
      on GitHub. Separate **provenance** (`ai_generated`) from **publication
      state**. On successful `W`, reconcile every posted inline finding with
      its real GitHub review-comment/thread identity (and general findings with
      the posted review/conversation representation), then enable the same
      `m`, `R`, `n`, and applicable `x` actions as reviewer-authored comments.
      A refresh or cache-hit reopen must retain that mapping and must not
      recreate a duplicate draft; failed or cancelled `W` must leave findings
      as local drafts. Add regression coverage for `A` → `W` → mark done →
      reply, including inline and summary-folded findings and a reopen after
      posting. Shipped by separating AI provenance (`ai_generated`) from
      publication (`ai_published`) and retaining the created review id plus
      each real inline comment id/thread id. A successful `W` now re-fetches
      the posted review, reconciles inline and summary-folded findings without
      changing their local triage state, and enables the normal
      done/reply/not-needed/resolve actions. Cache refresh carries those mapped
      findings forward while removing duplicate freshly-fetched GitHub
      representations; if the immediate identity fetch fails, publication is
      still recorded so retrying `W` cannot double-post and refresh can finish
      the mapping. New `A` runs replace only unposted drafts and allocate fresh
      synthetic ids above retained published findings. Regression tests cover
      inline + summary reconciliation, entering the Done reply flow, and a
      cache-backed refresh/reopen without duplicates. → `src/github.rs`,
      `src/app/pr_review.rs`, `src/app/tests.rs`,
      `src/db/pr_review_cache.rs`.

- [x] **Triage-session token/cost tracker.** The pane now snapshots the
      selected fix target's usage when a PR-triage visit begins and shows the
      live delta as `this visit N eff · $X` beside the existing cumulative
      session usage. A dedicated session created by the first fix starts at
      zero; switching to an already-running live session snapshots it at the
      moment it is selected, so unrelated earlier work is not charged to the
      triage visit. Returning with `P` or manually refreshing comments preserves
      the snapshot. Per-counter saturating subtraction handles corrected or
      rotated provider totals safely, and the narrow-header fallback prefers
      the visit tally when both totals do not fit. Focused tests cover growth,
      unchanged baselines, and saturating deltas. Stretch remains: per-comment
      cost breakdown and a per-PR cumulative total persisted in SQLite. →
      `src/app/state.rs`, `src/app/pr_review.rs`, `src/token_tracking.rs`,
      `src/ui/dashboard.rs`, `src/ui/dialogs/pr_review.rs`, `src/app/tests.rs`.

- [x] **Active-PR indicator on the dashboard.** Feature rows now show
      `[PR #321 · 4 open]` when their branch has an open pull request, with
      zero-open badges colored as complete and a number-only fallback when
      thread metadata is temporarily unavailable. A single non-overlapping
      background batch runs on the existing feature-status sync cadence,
      reusing `GhCli::resolve_pr` and `GhCli::review_threads`; rendering only
      reads the in-memory cache, so no `gh` process can block a dashboard
      frame. Cache entries carry the feature branch and PR head SHA, stale
      results are discarded after a branch change, confirmed no-PR results
      remove the badge, and transient GitHub failures preserve the last known
      value. Focused tests cover applying/removing/preserving cached results,
      rejecting stale-branch results, and rendering the badge with its open
      thread count. Stretch remains: color the badge by review state
      (changes-requested vs. approved vs. comments-only). → `src/app/sync.rs`,
      `src/app/mod.rs`, `src/main.rs`, `src/ui/list.rs`, `src/app/tests.rs`.

- [x] **Highlight the logged-in user's own PRs in the PR picker.** A new
      `GhCli::current_user` (`src/github.rs`, `gh api user -q .login`) — also
      now shared by `is_self_review`, which duplicated the same call inline —
      is resolved once per session via `App::resolve_gh_current_user` and
      memoized in a new `App::gh_current_user: Option<Option<String>>` field
      (outer `None` = not yet attempted; `Some(None)` = attempted and failed,
      also cached so an unauthenticated `gh` doesn't retry the call on every
      picker open/refresh). `open_pr_picker` resolves it once and stores the
      login on the new `PrPickerState::current_user` field; `pr_picker_row`
      (`src/ui/dialogs/pr_review.rs`) compares it case-insensitively against
      each entry's `author` and, on a match, bolds the `@author` span and adds
      a `you` chip — so triaging your own open PRs (the common case: fixing
      review comments left on work you authored) doesn't require reading
      every `@author` in the list. `pr_picker_toggle_closed` (`a`) re-fetches
      the entry list but leaves `current_user` untouched, since toggling the
      open/closed filter doesn't change who's logged in. Cheap and read-only —
      no new `gh` calls beyond the one made once per session. Unit-tested
      (`pr_picker_row` tags a matching author case-insensitively, leaves a
      non-matching author untagged, and stays untagged when the current user
      is unresolved). → `src/github.rs`, `src/app/mod.rs`,
      `src/app/pr_review.rs`, `src/app/state.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/app/tests.rs`.

- [x] **Rename the feature to "PR Triage."** "PR Comment Review" / "PR review"
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
  sessions. Shipped across the dashboard/help entry, pane and picker titles,
  loading/status labels, session-harness copy, README, and linked-session
  return badge. New dedicated sessions use the `"PR Triage"` label; lookup
  prefers that label but still recognizes the legacy `"PR Review"` label so
  an already-running session is reused across upgrades. Focused unit coverage
  verifies both current-label preference and legacy-label compatibility. →
  `src/app/pr_review.rs`, `src/ui/dialogs/pr_review.rs`,
  `src/ui/dialogs/help.rs`, `src/ui/status.rs`, `src/ui/dashboard.rs`,
  `src/github.rs`, `README.md`.

- [x] **Dedicated triage-session status badge in the PR Triage pane.** Once
      the dedicated session exists, the pane header now shows
      `[dedicated ● working]` while that exact agent session is thinking or
      running a tool, and `[dedicated idle]` when it is waiting/finished; no
      badge is shown before the first `f` creates the session. The existing
      hook/plugin messages already carry `amf_feature_session_id`, but AMF
      previously discarded that precision after using it for token-source
      binding and retained only feature-level `tmux_session` activity. New
      per-feature-session thinking/tool sets preserve those IDs through IPC,
      and `pr_review_dedicated_session_working` checks the current or legacy
      dedicated-session label against them. This avoids a blocking
      `capture-pane` probe and, crucially, prevents another agent window in the
      same feature from falsely lighting the badge. The local Codex
      prompt-submit fast path now records the exact feature-session ID too, so
      it does not wait for the later IPC round trip. Focused tests cover no
      badge before creation, idle/working transitions, and isolation from a
      different agent window in the same tmux session. → `src/app/mod.rs`,
      `src/app/notifications.rs`, `src/app/sync.rs`, `src/app/pr_review.rs`,
      `src/ui/dashboard.rs`, `src/ui/dialogs/pr_review.rs`,
      `src/app/tests.rs`.

## Open questions

None. The last one — whether to add a cross-project review-memory layer —
was decided in favor of merging one in, and shipped; see the Epic E
Progress item "Cross-project (global) review-memory layer".

Resolved and no longer tracked here (see the linked Epic item for the
decision and implementation): the cross-project review-memory layer (Epic E
"Cross-project (global) review-memory layer" — merged in, not isolated),
which agent session runs fixes (Epic B,
dedicated-session default), AI-authored content attribution (Epic D "AI
attribution on AMF-posted comments"), templated-reply channel disclosure
(Epic C "posted via AMF" footer), conversation-comment grouping (Epic D's
`PrSortMode::Conversations`), resolve-without-reply behavior (shipped as-is
— `R` stays independent of `r`), outdated-comment badging (shipped in
Epic D's pane-clarity item), review-memory rot (Epic E "Prevent
review-memory rot" — `c` in the PR picker), and whether AI review belongs in
this pane (Backlog "Split AI Review into its own workflow" — it doesn't;
shipped as a dedicated pane). GitLab/Bitbucket support is an explicit
non-goal for v1 (GitHub `gh` only), not an open question.

## Backlog

- [ ] **Let `c` compact the global review-memory doc too.** The cross-project
      layer shipped with a scope toggle on the two *append* flows (`M`, `b`)
      but not on the compact pass — `c` still always targets the project doc,
      which is where the decided scope of that work ended. So the global doc
      can grow indefinitely (via `M`'s global scope) with no pruning path
      short of hand-editing it, which is the same rot the project doc's
      compact pass exists to prevent. The fix is small and mirrors what's
      already there: a `scope` on `CompactConfirmState` toggled by `g`,
      re-reading `count_findings` for the newly selected doc on each toggle,
      and `review_memory_compact_confirm_run` resolving through
      `review_memory_paths(repo).for_scope(scope)` instead of `.project`.
      Worth watching first whether a hand-curated global doc actually rots
      the way an auto-fed project doc does — it's fed one deliberate `M` at a
      time, so it may not need the machinery. →
      `src/app/pr_review.rs`, `src/app/state.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`.
- [x] **Split AI Review into its own workflow (resolves the "does AI review
      belong in this pane" open question).** AMF's own review of a PR's diff
      (`A`/`W`) used to live inside PR Triage, converting each finding into a
      synthetic `PrComment` merged into the same list real GitHub comments
      live in — a fit that kept fighting the triage data model on every
      follow-up: a synthetic id range kept clear of real GitHub ids, a
      bot/human chip special-cased for a third "AI, not yet posted" kind, a
      `diff_hunk` reconstructed from the full diff instead of coming for
      free, and a background-job lifecycle that didn't compose with "merge
      into whichever pane the user is looking at" (the real bug that started
      this reconsideration — findings silently dropped after an `esc`).
      Shipped as a fully separate pane instead: a new `src/app/ai_review.rs`
      owns a first-class, persisted `AiReviewFinding` (`path`/`line`/`body`/
      `diff_hunk`/`skipped`/`published`) and the whole generate → poll →
      post lifecycle (moved verbatim from the old `pr_review.rs` methods,
      retargeted at a new `AppMode::AiReview(AiReviewState)` /
      `AppMode::AiReviewRunning(AiReviewRunState)` pair), with its own
      `ai_review_cache` SQLite table (migration 012) keyed by `PR# + head
      SHA` — no longer riding inside `pr_review_cache`'s blob. The shape is
      deliberately **lean**: `W` posts every kept (not skipped, not already
      posted) finding as one GitHub review and simply marks them
      `published` locally — no reconciliation of GitHub identities back onto
      the pane (the `reconcile_ai_publication`/`carry_forward_ai_drafts`
      machinery that used to keep posted drafts alive in the triage list is
      gone entirely). Following up on a posted finding (mark done, reply,
      resolve, inject a fix) happens back in PR Triage once a manual refresh
      picks it up as an ordinary fetched comment. New UI lives in
      `src/ui/dialogs/ai_review.rs` (findings list + detail, the running
      screen, harness/model pickers, post-confirm dialog — all moved out of
      `dialogs/pr_review.rs`, which lost every AI-only code path: the
      `[AI]`/`ai` chips, the header throbber/last-run badge, and the
      `ai_generated`/`ai_published` fields on `PrComment`). Four entry
      points, all wired to the same `App::open_ai_review_for_pr`: PR
      Triage's `A` key (`open_ai_review_from_triage`, which additionally
      stashes the triage pane so closing AI Review returns to it — the only
      entry that does; the other three close straight to the dashboard,
      matching how `close_pr_review` already behaves) — dashboard `W`
      (`open_ai_review`), leader `W` from inside an agent session
      (`open_ai_review_from_view`, peer to `leader G`), and `W` in the PR
      picker (`pr_picker_choose_ai_review`, peer to `Enter`'s
      `pr_picker_choose`). Unit-tested (parsing/prompt/build-review tests
      carried over into `ai_review.rs`'s own test module; App-level tests
      for all four entry points, the triage-pane stash/restore round trip,
      the `AiReviewState`-shaped `has_visible_animation`/
      `invalidate_pr_context_for_transition` regressions, and the
      `ai_review_cache` DB round trip) — full suite green (1126 tests),
      `cargo clippy`/`cargo fmt` clean. No migration needed: old
      `pr_review_cache` rows with `ai_generated`/`ai_published`/
      `last_ai_review` keys deserialize fine (serde ignores unknown fields);
      their same-SHA AI drafts don't carry into the new table, so
      regenerating with `A` after upgrading is the norm. →
      `src/app/ai_review.rs`, `src/app/state.rs`, `src/app/mod.rs`,
      `src/app/pr_review.rs`, `src/app/navigation.rs`, `src/db/ai_review_cache.rs`,
      `src/db/migrations.rs`, `src/db/mod.rs`, `src/db/pr_review_cache.rs`,
      `src/handlers/ai_review.rs`, `src/handlers/mod.rs`,
      `src/handlers/pr_review.rs`, `src/handlers/normal.rs`,
      `src/handlers/view.rs`, `src/ui/dialogs/ai_review.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/ui/dialogs/mod.rs`,
      `src/ui/dialogs/help.rs`, `src/ui/dashboard.rs`, `src/ui/status.rs`,
      `src/ui/pane.rs`, `src/app/tests.rs`, `README.md`, `CHANGELOG.md`.

      **Follow-up, same day — from real use.** Splitting the pane dropped a
      real signal: PR Triage's own header used to show "AI review running…"
      / the last run's outcome (it lived inside `draw_pr_review` before the
      split), and that disappeared along with the rest of the AI-only
      rendering — leaving PR Triage's header with no idea a background
      review for the same PR was still going once you left the AI Review
      pane (via `leader W`, say) and came back in through `G`. The
      dashboard/session ambient badge (`pr_triage_badge_span`) still knew,
      via the unchanged `ai_review_running_for_workdir`, just not the pane
      itself. Fixed by threading that same already-live check into
      `draw_pr_review` as a new `ai_review_running: bool` parameter (the
      dashboard's `PrReview` draw call site computes it from `app.mode`'s
      `state.workdir` before the mutable borrow, same pattern as
      `dedicated_session_working`), rendering a `[AI review running]` badge
      next to the existing `[dedicated ● working]` one. Render tests cover
      both the present and absent cases. → `src/ui/dashboard.rs`,
      `src/ui/dialogs/pr_review.rs`, `CHANGELOG.md`.

      **Follow-up, same day — expose progress for long AI reviews.** Real use
      on PR #473 made a large Codex-backed review look stuck: the diff was
      roughly 386 KB / 95–100k estimated tokens, while `HeadlessRunner`
      buffered all output with `wait_with_output()` and the running pane could
      only say "Reviewing diff…" until the process exited. Worse, closing AMF
      during that wait orphaned the ephemeral `codex exec` process and left no
      result to recover. Codex-backed AI reviews now opt into the CLI's
      structured `--json` event stream, drain it while the process is alive,
      preserve the final `agent_message` for the existing finding parser, and
      reduce intermediate events to safe activity labels (no prompt,
      reasoning text, or raw command content). `AiReviewRunState` carries the
      latest activity, elapsed start time, and reported usage into the running
      pane; non-Codex harnesses keep their existing runner and still gain the
      elapsed timer. Tests cover JSON flag/model/stdin ordering, final-message
      extraction, error and usage events, progress redaction, app-state
      polling, and the rendered running pane. Full suite green (1134 passed,
      1 ignored); `cargo check`, strict clippy, and formatting clean. Restart
      recovery remains separate follow-up work: an in-flight review still
      belongs to the AMF process that launched it. → `src/headless.rs`,
      `src/app/ai_review.rs`, `src/app/state.rs`,
      `src/ui/dialogs/ai_review.rs`, `src/app/tests.rs`, `CHANGELOG.md`.

      **Follow-up — make progress reopenable.** The first progress pass kept
      its timer and activity only inside `AiReviewRunState`, so `Esc` discarded
      the detailed view even though the headless review continued and the
      findings-pane header still said `running…`. Live run progress now also
      lives on `App`, updates regardless of the current pane, and is cleared
      with the receiver on completion/invalidation. While that run exists the
      findings footer says `A view progress`; pressing `A` reconstructs
      `AiReviewRunning` with the original start time, latest stage/activity,
      and usage rather than trying to start a duplicate pass. App and render
      tests cover progress arriving while away and the return path; full
      suite green (1135 passed, 1 ignored), with check, strict clippy, and
      formatting clean. →
      `src/app/mod.rs`, `src/app/state.rs`, `src/app/ai_review.rs`,
      `src/app/pr_review.rs`, `src/app/tests.rs`,
      `src/ui/dialogs/ai_review.rs`, `CHANGELOG.md`.

      **Follow-up — live progress for every built-in harness.** The progress
      screen no longer falls back to elapsed time alone when AI Review uses
      Claude, Opencode, or Pi. Each runner now consumes that CLI's structured
      event stream and maps lifecycle, reasoning, tool, completion, retry,
      and usage events onto the same safe activity labels already used for
      Codex. Prompts still travel over stdin, and raw reasoning, commands,
      tool arguments/results, and draft response text never enter progress
      state. Harness selection validates the required streaming capability
      up front and gives older CLI versions an upgrade hint. Provider fixture
      tests cover argument selection, final-response extraction, cumulative
      usage, error handling, and redaction. Restart recovery remains separate
      follow-up work. → `src/headless.rs`, `src/app/ai_review.rs`,
      `CHANGELOG.md`.

- [x] **Remove F keybind (queue-marked fixes) — redundant with B.** `F` queued
  every marked comment's fix into the review session immediately (auto-submit
  each). `B` opens a confirm dialog before combining them into one prompt and
  launching. Both advance a batch of marked items, but `B` lets the user
  review/edit before committing, while `F` is fire-and-forget with no
  visibility. Real use found `F` not useful — users prefer the confirm
  visibility of `B`. Removed the handler, queueing implementation, help/footer
  hints, README guidance, and obsolete tests; `Space` + `B` is now the only
  marked-comment batch workflow. → `src/handlers/pr_review.rs`,
  `src/app/pr_review.rs`, `src/ui/dialogs/pr_review.rs`,
  `src/ui/dialogs/help.rs`, `src/app/tests.rs`, `README.md`, `CHANGELOG.md`.

- [x] **Keymap audit — too many bindings.** The PR triage pane has accumulated
  many keybinds (`f`, `B`, `r`, `n`, `R`, `x`, `o`, `P`, `M`, `W`, `A`,
  `i`, `space`, `#`, `g`, `a`, `G`, `h`, `j/k`, etc.). Real use reports the
  pane is hard to use and overwhelming. Audit every binding: is it needed? Can
  it be removed or merged with another? Can the workflow be simplified to
  require fewer keys and fewer modes/dialogs? Example: can `r`/`n` be merged
  into a single "reply" key that prompts for the reply kind (fix/not-needed)?
  Can `A`/`W` (AI review) be deferred or simplified? Prioritize discoverability
  and lean keymaps over feature breadth.

  **Audit (proposal — no code changed yet, scope decision needed before
  implementing).** Full inventory of the main pane's top-level keys, from
  `handle_pr_review_key` (`src/handlers/pr_review.rs`), beyond the
  unavoidable `j/k`/arrows/`Esc`/`q`/`Ctrl+d`/`Ctrl+u`/`PageUp`/`PageDown`:

  | Key | Action | Needed? | Notes |
  | --- | --- | --- | --- |
  | `h` | hide/show resolved | Keep | Common triage filter, cheap toggle, no overlap. |
  | `o` | cycle sort order | Keep | Distinct axis from `h`; five modes already justified by Epic D. |
  | `f` | inject fix (single) | Keep | Core loop, can't be merged away. |
  | `t` | toggle fix target (dedicated ↔ existing-live) | **Merge candidate** | A per-PR, set-once-then-forget choice (footer already shows the current target). Fits better as a step inside the harness/fix-confirm dialog than a standalone always-live key. |
  | `P` | peek dedicated/live session, round-trip back | Keep | Core loop counterpart to `f`; this is the fix for the "stuck watching the agent" friction Epic D solved. |
  | `space` | mark for batch | Keep | Required by `B`. |
  | `B` | inject combined batch prompt | Keep | Core loop, the safer alternative to the removed `F`. |
  | `R` | reply "Done in `<sha>`" | **Merge candidate** | See below. |
  | `n` | reply "not needed" | **Merge candidate** | See below. |
  | `M` | add selected comment to review-memory | Keep | Low-frequency but distinct destination (repo file, not GitHub) — folding into `R`/`n` would blur "reply to GitHub" with "note for next time." |
  | `m` | mark done (local triage only) | Keep, but audit overlap with `R` | `m` is silent bookkeeping; `R` both posts to GitHub *and* marks done. In practice `R` supersedes `m` for the common "I fixed it" case — `m` is really for "handled some other way, no reply warranted." Rename/relabel for clarity rather than remove. |
  | `x` | resolve/reopen GitHub thread | Keep | Different system (GraphQL thread state) from `m`/`R`; already explicitly independent of replying by design (Epic C). |
  | `s` | skip (local only) | Keep | Distinct from `n` (skip leaves no GitHub trace; `n` posts an explanation). |
  | `r` | refresh | Keep | Necessary escape hatch after a push; low collision risk once `R`/`n` merge frees a letter. |
  | `i` | install syntax highlighting for selected file | **Move candidate** | Rare, one-time-per-language action; parity with the diff viewer doesn't require equal keymap prominence. Could live behind a secondary/help-only affordance instead of a top-level letter. |
  | `g` | switch to a different PR (opens PR picker) | Keep | Not truly redundant with the dashboard's `G` — this is reachable without leaving the pane, which is the whole point of the ambient-indicator/leader-`G` work in Epic D's "Nice to have" section. |
  | `A` | run AI review of diff | Keep, but gate behind `W`'s outcome more | Genuinely token-spending and optional per the original design (opt-in, token preview) — already about as minimal as the workflow allows. |
  | `W` | post AI-review draft as GitHub review | Keep | Necessary second step after `A`; conflating generate+post would remove the "review before spending a write" gate that's core to the feature's design. |

  **Concrete first-pass proposal (lowest risk, reversible):**
  1. Drop `t` as a standalone key. Fold the dedicated/existing-live choice
     into the harness-pick dialog (which already gates the *first* `f`/`B`
     of a PR) as an explicit "fix target" row, so the choice is made once,
     at the point it's needed, instead of living as an always-on toggle key
     competing for attention with the 17 others.
  2. Merge `R`/`n` into a single `R` ("reply") key. Pressing it opens the
     existing reply dialog one level earlier: a two-item kind picker
     ("Done in `<sha>`" / "Not needed"), reusing the same downstream
     edit/confirm/post flow both templates already share
     (`handle_reply_key`). This mirrors the plan's own original suggestion
     for this exact pair and frees a letter (`n`) without losing either
     template.
  3. Leave `i` where it is for now (low priority, not part of the
     "overwhelming" complaint's likely cause) — revisit only if the two
     changes above don't feel sufficient in practice.

  **Deliberately not proposed:** collapsing `m`/`s`/`x` (they span three
  different systems: local triage, local skip, GitHub thread resolution —
  merging them risks silently doing a GitHub write when the user only meant
  local bookkeeping) or `A`/`W` (removing the generate/post split removes the
  explicit-approval gate before any GitHub write, which is a deliberate
  design constraint from the top of this doc, not incidental complexity).

  Scope/priority for actually implementing this decided with the user
  2026-07-16: write up the audit first (this entry), pick specific merges to
  implement in a follow-up rather than doing a broad pass blind.

  **Shipped, 2026-07-16 — all three merges, including the one marked
  "deliberately not proposed" above.** After seeing the audit, the user
  asked for `t` dropped and `R`/`n` merged as proposed, **and** for
  `m`/`s`/`x` to get the same treatment (with the GitHub-write row
  explicitly labeled as such) — overriding the audit's caution about mixing
  local and remote state in one menu.
  - **`t` removed.** `HarnessPickState` (`src/app/state.rs`) now carries
    `rows: Vec<FixTargetPickRow>` (`ExistingLive` plus one `Dedicated(AgentKind)`
    row per allowed harness) instead of a bare agent list, defaulting the
    highlight to the dedicated row for the project's preferred agent — so the
    first `f`/`B` of a pane visit always resolves the fix target, not just the
    harness. A new `PrReviewState::fix_target_picked` flag (plus the existing
    `review_harness.is_some()` check, kept for back-compat with tests/paths
    that set it directly) gates re-asking; picking a row goes through a new
    `App::pr_review_set_fix_target` that snapshots the newly-targeted
    session's usage baseline the same way the old toggle did. →
    `src/app/pr_review.rs`, `src/app/state.rs`, `src/ui/dialogs/pr_review.rs`.
  - **`R`/`n` merged.** A new `ReplyKindPickState` + `ReplyKind::ALL`/`menu_label`
    back a two-row picker opened by `R` (`App::pr_review_open_reply_pick`);
    confirming it dispatches into the existing `pr_review_open_reply_done`/
    `pr_review_open_reply_not_needed` unchanged, so the downstream edit/confirm/
    post flow is untouched. → `src/app/pr_review.rs`, `src/app/state.rs`,
    `src/handlers/pr_review.rs`, `src/ui/dialogs/pr_review.rs`.
  - **`m`/`s`/`x` merged.** A new `MarkPickState` + `MarkAction::ALL`/`menu_label`
    back a three-row picker opened by `m` (`App::pr_review_open_mark_pick`):
    Done (local), Skip (local), and **"Resolve/Reopen thread on GitHub"** —
    the label names GitHub explicitly and the row renders in the theme's
    warning color, distinct from the two local-only rows above it, so the one
    action that writes anywhere outside AMF doesn't read as just another local
    toggle. Confirming dispatches into the existing `pr_review_mark_done`/
    `pr_review_skip`/`pr_review_toggle_resolve` unchanged — applied immediately,
    no further confirm step, matching the original single-key behavior since
    none of the three need editable text. → `src/app/pr_review.rs`,
    `src/app/state.rs`, `src/handlers/pr_review.rs`, `src/ui/dialogs/pr_review.rs`.
  - Net: 6 top-level keys (`t`, `R`, `n`, `m`, `s`, `x`) become 2 (`R`, `m`),
    each one keypress plus a single-selection picker away from the same
    action as before — no workflow removed. Footer/help text
    (`src/ui/dialogs/help.rs`, `README.md`, `CHANGELOG.md`) updated to match.
    Unit-tested (picker open/move/confirm/cancel for both new pickers, the
    fix-target row labels, and `MarkAction`/`ReplyKind` menu-label text
    reflecting current triage/resolution state) — full suite green
    (1133 tests) and `cargo clippy` clean. `i`/`A`/`W` left untouched per the
    audit.

- [x] **Name the existing session in the `f`/`B` fix-target picker.** The
      target menu's `Existing live session` row was a fixed string with no
      way to tell which running session a fix would land in. `FixTargetPickRow`
      now carries the resolved label (`ExistingLive(Option<String>)`);
      `pr_review_open_harness_pick` resolves it via the same
      `pr_triage_session_index(feature, FixTarget::ExistingLive)` lookup the
      rest of the fix-target machinery already uses, so the row reads
      `Existing live session (Claude 2)` once a live agent session exists.
      Falls back to the unadorned `Existing live session` when none does yet
      (e.g. the first fix of a stopped feature) — same picker, same lookup,
      for both the single-fix (`f`) and combined-batch (`B`) flows, since both
      resolve through this one menu. Unit-tested (`label()` for both the named
      and fallback cases) plus render coverage for `draw_harness_pick` proving
      the resolved name and the fallback actually appear in the rendered
      picker. → `src/app/pr_review.rs`, `src/app/tests.rs`,
      `src/ui/dialogs/pr_review.rs`.

- [x] **AI review result persistence (UX — visibility).** When running an AI
      review (`A`), the user could escape back to the pane, navigate away, or
      close AMF entirely; returning to the PR later showed no indication of
      what happened — no visual marker that a review ran, no success/error
      signal, no "zero findings" message. The result was already cached in
      `pr_review_cache`, but nothing surfaced it. A new `PrReview::last_ai_review:
      Option<AiReviewRun>` field (`ran_at` + an `AiReviewRunOutcome::Findings(n)`
      / `Error(String)` outcome) is set by `poll_ai_pr_review_bg` on **both** the
      success and error paths (previously only success touched the cache) and
      cached via the existing `cache_pr_review` write, so it round-trips through
      `pr_review_cache` like the rest of the review and needs no new table
      (`#[serde(default)]` for backward-compat with pre-existing cache rows).
      `carry_forward_ai_drafts` — already the mechanism that keeps AI drafts
      alive across a same-head-SHA manual refresh — now also carries the
      record forward, since its cache lookup is already keyed by the same
      `PR# + head SHA`; a genuinely new SHA (the PR moved) naturally starts
      with no record, matching the existing findings going stale at the same
      point. The pane header shows a badge whenever the running screen isn't
      already covering it: `AI review: N findings (5m)`, `AI review: no
      findings (2h)`, or `AI review failed (1h): <truncated error>` in the
      danger color. → `src/app/pr_review.rs`, `src/db/pr_review_cache.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/app/tests.rs`, `CHANGELOG.md`.

- [x] **"Done in `<sha>`" reply: smarter commit detection (UX).** `R` used to
      seed "Done in `<sha>`" from `HEAD` blindly, with no check that the
      commit actually addressed the comment being replied to. A new
      `commit_for_done_reply` (`src/app/pr_review.rs`) tries, in order: (1)
      `git log -L<line>,<line>:<path> -1 --format=%h --no-patch` — the line's
      own history — skipped when the comment's anchor is `outdated` (a
      stale/shifted line number would just find noise); (2) `git log -1
      --format=%h -- <path>` — the file's most recent commit, when the line
      search finds nothing or doesn't apply (file-level comment, outdated
      anchor); (3) bare `HEAD` (`latest_commit_short_sha`, unchanged) as the
      last resort. Only the bare-`HEAD` fallback gets a caveat: the seeded
      reply is `Done in \`<sha>\` (latest commit).` instead of the confident
      `Done in \`<sha>\`.` when a plausible match was found — so the
      still-editable template signals when it's guessing. (Letting the user
      pick from several candidate commits was the documented alternative;
      went with the single-best-guess fallback chain instead since it needs
      no new UI and the reply stays editable either way.) Unit-tested at both
      layers: `commit_touching_line`/`commit_touching_file`/
      `commit_for_done_reply` against real throwaway git repos (line-specific
      match, out-of-range line, untracked path, outdated-anchor skip,
      bare-HEAD caveat, no-repo), plus two `App`-level tests driving `R`
      end-to-end against a real repo (line-history match, bare-HEAD caveat).
      → `src/app/pr_review.rs`, `src/app/tests.rs`, `README.md`,
      `CHANGELOG.md`.

- [x] **Model selection for AI review, independent of the working harness
  (flexibility).** AI review (`A`, and Epic E's lookback distill) used to run
  with no way to pick a different model — e.g. a stronger/slower model to
  catch subtleties, independent of whatever the feature's working session
  uses. A new `AppConfig::review_model: Option<String>` is passed as an
  explicit `--model <name>` to the headless CLI for both `A` and the
  lookback bootstrap, falling back to the harness's own default model when
  unset (`None`, the default). `HeadlessRunner::run` (`src/headless.rs`)
  gained the `model: Option<&str>` parameter — `HeadlessCommand` now splits
  fixed `args` from a `trailing` tail (Codex's `-` stdin marker) so an
  inserted `--model <name>` lands before it rather than after; a new
  `supports_model_flag` gates this to Claude/Codex/Opencode, since Pi's
  headless model flag isn't verified (mirrors the existing Pi caution in
  `check_available`/`select_for_interview`) — a requested model is silently
  not applied there rather than guessed at. The lookback bootstrap
  (`run_review_memory_bootstrap`) previously bypassed `HeadlessRunner`
  entirely, hardcoded to `ClaudeLauncher::run_headless` with no
  harness-independent model support; it now routes through
  `HeadlessRunner::run(&AgentKind::Claude, ...)` too, so `review_model`
  covers both paid-pass call sites the item names. On the "how large a diff"
  question: went with the same soft-ceiling pattern as the combined
  fix-prompt batch rather than chunking or refusing — a new
  `AI_REVIEW_PROMPT_TOKEN_WARN` (40k tokens) fires a one-time warning toast
  once the assembled review prompt's token estimate is known, but the review
  still runs; chunking isn't worth the complexity until real use shows it's
  needed. Unit-tested (`assemble_args` ordering/omission per harness in
  `headless.rs`; the toast fires above the ceiling and stays silent below
  it, via the existing `poll_ai_pr_review_bg` `Reviewing`-stage test
  fixture). Reply drafting was dropped in favor of deterministic templates
  (Epic C), so this only ever applied to AI review, not replies. →
  `src/headless.rs`, `src/app/mod.rs`, `src/app/pr_review.rs`,
  `src/app/tests.rs`.

  **Follow-up, same day — an in-pane picker, not just a config setting.**
  A config-only knob turned out to be the wrong shape: real use expected a
  picker like the existing `A` harness picker, not an edit-config.json step.
  A new single-select `AiModelPickState`/`ModelPickRow` (`src/app/state.rs`)
  opens automatically right after the harness is chosen (`start_ai_pr_review`
  now checks a new `PrReviewState::ai_review_model_picked` flag the same way
  it already checked `ai_review_harness`), offering `Default` (harness's own
  model), verified presets (`model_pick_rows` — currently only Claude's
  `sonnet`/`opus`/`haiku`/`fable`, confirmed against `claude --help`; other
  harnesses get just `Default`/`Custom` rather than guessed-at aliases), and
  `Custom…` for a free-typed model name/id (`⏎` opens a text field, a second
  `⏎` submits it — blank falls back to `Default`; `esc` while typing returns
  to the list without losing what's typed, a second `esc` cancels the picker
  outright with the harness still remembered). The choice is remembered on
  `PrReviewState::ai_review_model` for the rest of the PR, same lifecycle as
  `ai_review_harness`; `begin_ai_pr_review` prefers it over the `review_model`
  config default (which now seeds the picker's initial highlight/selection
  instead of being the only way to set it — matching an existing preset
  exactly highlights that row, otherwise `Custom` opens pre-filled with it,
  so an existing config value is never silently hidden). Pi skips the picker
  entirely and proceeds straight to the review, since it has no verified
  model flag to offer. The lookback bootstrap has no harness-picker step to
  hang a model picker off of, so it's unchanged (still config-only). New
  `AppMode`-level key handling in `handlers/pr_review.rs`
  (`handle_ai_model_pick_key`) and a `draw_ai_model_pick` dialog in
  `ui/dialogs/pr_review.rs`, mirroring the harness picker's shape. Unit- and
  live-tested: 9 new tests cover the picker opening after the harness step,
  Pi's skip, default/preset/custom selection, the custom row's two-step
  confirm and blank-falls-back-to-default behavior, and esc's two levels
  (back-to-list vs. full cancel); confirmed live by building the binary and
  driving it in an isolated tmux server + isolated `HOME` against this
  repo's own PR (`#466`) — the harness picker, the new model picker with all
  six rows, custom text entry, both esc levels, and the harness-remembered
  skip on a second `A` all matched the design. → `src/app/state.rs`,
  `src/app/pr_review.rs`, `src/handlers/pr_review.rs`,
  `src/ui/dialogs/pr_review.rs`, `src/app/tests.rs`.

- [x] **BUG — fix injection can silently target the wrong checked-out
  branch when triaging a manually-picked PR (correctness).** `G`'s picker (and
  `g`/`#` inside the pane) let the user open *any* PR from the repo, not just
  the one for the feature's own checked-out branch, and `f`/`B` fix injection
  reads files from `state.workdir` regardless of which PR is loaded — a
  mismatch meant a fix could silently land on the wrong branch. (`A`/AI
  review was confirmed unaffected: it feeds the agent the diff as inline text
  from `gh pr diff`, resolved by PR number against the GitHub API, never
  touching the working tree.) Fixed by threading `headRefName` through PR
  resolution: `PrRef` gained a `head_ref: String` field (`#[serde(default)]`
  so pre-existing `pr_review_cache` rows still deserialize), populated by
  `resolve_pr`, `fetch_pr_by_number`, and the branch-scoped `gh pr view`
  query (`src/github.rs`). `PrReviewState` snapshots the workdir's actual
  branch (`WorktreeManager::current_branch`) into a new
  `checked_out_branch: Option<String>` field whenever the pane is
  entered/refreshed (cache-hit and background-fetch paths), and a new
  `PrReviewState::branch_mismatch()` (delegating to a pure, unit-tested free
  function in `src/app/state.rs`) compares the two, `None` when they match or
  either side is unknown. Surfaced two ways: an always-visible danger-colored
  pane-header banner ("reviewing PR for branch `X`, but this worktree is on
  `Y` — fixes will be applied to `Y`, not `X`") in `draw_pr_review`, and the
  same warning inside the fix confirm/edit dialog (`draw_fix_confirm`,
  repurposing its existing spacer line) — since `⏎` from that dialog is
  already the explicit-acknowledge gate before anything is injected, no new
  modal or blocking step was needed. Unit-tested (`head_ref` JSON
  parsing/defaulting in `github.rs`; `branch_mismatch`'s match/differ/unknown
  cases in `state.rs`; full-frame render tests proving the header banner and
  the fix-confirm warning appear only on a mismatch, in `ui/dashboard.rs` and
  `ui/dialogs/pr_review.rs`). → `src/github.rs`, `src/app/state.rs`,
  `src/app/pr_review.rs`, `src/ui/dialogs/pr_review.rs`, `src/db/pr_review_cache.rs`,
  `src/app/tests.rs`, `src/ui/dashboard.rs`.

- [x] **Open PR Triage from inside the agent harness session, with an ambient
      status indicator (discoverability).** `leader G` — peer to `leader p`
      (prompt library) in `LEADER_COMMANDS` — now opens PR Triage directly
      from `AppMode::Viewing`, no prior dashboard visit required. It resolves
      the live session's feature via a new `App::feature_for_view` (mirrors
      the existing project/feature-by-name lookup pattern used by
      `trigger_final_review` et al., `src/app/navigation.rs`) and hands the
      workdir to a new shared `open_pr_review_for_workdir`, factored out of
      `open_pr_review` so the dashboard's `G` and the new `open_pr_review_from_view`
      both run the same `gh` preconditions → resolve → enter-pane (or
      fall through to the PR picker on `NoPrForBranch`) logic. Pairs with an
      ambient badge in the existing Viewing-mode top-right badge row
      (`src/ui/dashboard.rs`, alongside remote-control / direct-input /
      back-to-triage): `[PR #N · M open]` — reusing the dashboard's own
      `active_pr_for_feature` sync/cache exactly as the feature-list row does
      — with `· ● working` appended via a new workdir-scoped
      `dedicated_review_session_working_for_workdir` (`pr_review_dedicated_session_working`
      refactored to delegate to it) and `· AI review` appended via a new
      `ai_review_running_for_workdir`, which checks `ai_review_pending`'s own
      stashed workdir rather than assuming `self.mode` still points at the
      pane that started the background job (the background AI review can
      outlive being `esc`-ed away from). The badge only renders for
      agent-harness views, matching the existing back-to-triage badge, and is
      simply absent when the feature has no active PR. Keybinding-help entry
      (`src/ui/dialogs/help.rs`). Unit-tested (`feature_for_view` resolves/
      misses by name; `open_pr_review_from_view` no-ops outside `Viewing` and
      shows a message for an unresolvable feature without touching `gh`;
      `dedicated_review_session_working_for_workdir` reads correctly while
      `self.mode` is `Viewing`, not `PrReview`; `ai_review_running_for_workdir`
      matches only the pending review's own workdir). Confirmed live: built
      the binary, drove it in an isolated tmux server + isolated `HOME`
      against a throwaway project/feature — the leader menu lists `G  PR
      Triage`, `leader G` from inside a real Claude session resolves the
      feature and (with `gh` unauthenticated in the isolated env) surfaces
      the same actionable error the dashboard path shows, staying in
      `Viewing` rather than kicking back to the dashboard; with `gh`
      authenticated, a branch with no open PR correctly fell through to the
      PR picker, and opening a PR from it loaded the full triage pane. →
      `src/app/navigation.rs`, `src/app/pr_review.rs`, `src/ui/pane.rs`,
      `src/handlers/view.rs`, `src/ui/dashboard.rs`, `src/ui/dialogs/help.rs`,
      `src/app/tests.rs`, `CHANGELOG.md`.

      **Follow-up, same day — route the badge into the sidebar box when one's
      showing.** The top-right badge and the `Claude`/`Codex`/`Opencode`
      sidebar (`leader b`) both fight for the same header real estate; asked
      for directly, so the ambient indicator now picks whichever surface
      isn't already spoken for. A new `AgentSidebarData::pr_triage_text`
      field (`src/ui/pane.rs`) renders as a "PR Triage" sidebar section —
      same `Label: value` line convention as `Status`/`Work` (`sidebar_value_style`
      bolds `working`/`running` values) — built by a new
      `pr_triage_sidebar_text(app, feature)` (`src/ui/dashboard.rs`) that
      composes the exact same three primitives the badge already used
      (`active_pr_for_feature`, `dedicated_review_session_working_for_workdir`,
      `ai_review_running_for_workdir`), so the two surfaces can never drift
      out of sync — only their layout differs. The top-right badge block
      gained a `!view.sidebar_visible` guard so it only renders when the
      sidebar is hidden; toggling `leader b` mid-session flips which one is
      showing. Unit-tested (`pr_triage_sidebar_text` composition incl. the
      open-count/no-count and working/AI-review line variants; the "PR
      Triage" section renders in `pane::draw` when present and is absent
      when not; a full `dashboard::draw()` render in `AppMode::Viewing`
      proves the badge and the sidebar box are mutually exclusive on
      `sidebar_visible`). Confirmed live: sidebar toggled cleanly with
      `leader b` in a real Claude session with no regressions (verified
      against a feature with no active PR — the render-test suite covers the
      active-PR content itself). → `src/ui/pane.rs`, `src/ui/dashboard.rs`,
      `CHANGELOG.md`.

      **Second follow-up, same day — two real-use bugs: the badge vanished
      while composing, and the underlying data could go stale (or never
      populate) inside a session at all.** (1) Compose interception is on by
      default, so entering a session normally lands straight in
      `AppMode::Compose`, not `AppMode::Viewing` — and the badge code lived
      only in the `Viewing` arm of `draw()`, so it silently never ran while
      composing (the common case). Fixed by extracting `pr_triage_badge_span`
      / `draw_badge_row` (`src/ui/dashboard.rs`) and calling them from both
      arms — in `Compose`, after `draw_mode_context_bar` rather than before
      it, since that call clears and redraws the whole top row for its
      breadcrumb and would otherwise wipe a badge drawn earlier in the same
      frame (caught by a new `compose_mode_still_shows_the_ambient_pr_badge_with_sidebar_hidden`
      render test, which failed against the naive before-context-bar
      placement). (2) Root cause of the emptier bug: `App::sync_active_prs_background`
      — the job that populates `active_prs`, the data both the badge and the
      sidebar box read — was only invoked from `main.rs`'s `!is_viewing`
      branch, bundled with tmux status reconciliation. Opening a PR for a
      feature while already inside its session (or simply staying in one)
      meant the indicator's data source never ran again, so it stayed empty
      or stale indefinitely — directly defeating the point of an ambient
      indicator meant to be read *without leaving the session*. Fixed by
      giving it an independent cadence/timer (`last_active_pr_sync`) with no
      `is_viewing` gate, reusing the same reentrancy guard
      (`active_pr_bg.is_some()`) so it stays a cheap, non-blocking background
      kick-off regardless of how often the check runs. → `src/ui/dashboard.rs`,
      `src/main.rs`, `CHANGELOG.md`.

      **Third follow-up, same day — sidebar box title hint.** The "PR
      Triage" section's border now carries a `<leader G>` hint (right-aligned
      in its top border), matching the existing `<leader l>` hint on the
      "Prompt" section — the same discoverability pattern, just pointing at
      the shortcut that opens the pane this box is a preview of. → `src/ui/pane.rs`.

- [x] **Headless/agent-posted fixes need concrete UI around reply posting and
      thread state (UX — visibility).** Surfaced while fixing three review
      comments on a branch: an already-running headless/automated path had
      posted `Done in <sha>` replies to all three threads, but the `<sha>` it
      named was the **original commit the comment was filed against**, not a
      commit that actually applied the fix — because the fix hadn't landed
      yet when the reply went out. Nothing in AMF (or the reply itself)
      distinguished "reply posted" from "reply posted *and confirmed
      accurate*," and after the real fix commit landed, confirming that the
      threads were in fact resolved required shelling out to `gh api
      graphql` for `isOutdated`/`isResolved` by hand — the existing
      `[outdated]` chip (Epic D pane-clarity item, `normalize` in
      `src/app/pr_review.rs`) only reflects the state of *incoming* review
      comments, not of AMF's own posted replies, and isn't re-checked after a
      reply goes out. Two gaps worth closing together: (1) the headless
      reply-posting path should hold off (or clearly caveat, the way the
      interactive `R` flow's `commit_for_done_reply` already caveats a
      bare-`HEAD` guess) until it can confirm the named commit actually
      touches the comment's anchor, rather than firing eagerly against
      whatever `HEAD` was at trigger time; (2) once a reply is posted —
      headless or via `R` — the pane should show its post status (posted /
      failed, and later, once GitHub reprocesses it, outdated/resolved) next
      to the reply itself, so confirming a fix landed correctly is a glance
      in AMF instead of a manual GraphQL query.

      **Scoped and shipped as two pieces, since the "headless path" that
      posted the inaccurate reply isn't AMF code at all** — grep confirms
      `Done in \`<sha>\`` is built in exactly one place
      (`PrComment`'s `R`-flow reply seeding, `src/app/pr_review.rs`), so the
      "headless/automated path" from the report is an agent working in a PR
      Triage / `pr-continue` session, using its own `gh`/bash access to post a
      reply on its own initiative — outside any AMF-owned code path. (1)
      `.claude/commands/amf/pr-continue.md`, the skill this repo already ships
      for "continue work on the PR by addressing review feedback," gained an
      explicit instruction not to post a "done" GitHub reply on its own
      initiative, and — if asked to reply — to reference a commit only after
      it's pushed and confirmed (`git show <sha> -- <path>`) to touch the
      comment's file, mirroring `commit_for_done_reply`'s own caveat logic
      rather than assuming `HEAD` addressed every open comment; it also points
      at AMF's own PR Triage pane as the preferred reply-posting path, since
      that one already derives and caveats the commit. (2) Because a
      headless-posted reply leaves **no local triage record** (it never went
      through `pr_review_post_reply`), "failed" isn't a state AMF can observe
      for it — there's no local attempt to have failed. What AMF *can* surface
      irrespective of who posted the reply: a new `PrComment::replies_in(&self,
      all: &[PrComment])` finds every already-fetched comment whose
      `in_reply_to` targets the selected one (GitHub inline replies always
      target the thread root directly, so this is a flat filter, not a chain
      walk), and the detail pane's new **Replies** section renders each one —
      author, a `[via AMF]` chip when the reply carries the "posted via AMF"
      channel-disclosure footer (`reply_posted_via_amf`, the only local signal
      distinguishing AMF's own post from anyone else's) and, per reply, its
      thread's live `[outdated]`/`[✓ resolved]` chips — right on the original
      comment. Previously this state only existed as a separate, easy-to-miss
      entry lower in the flat comment list; now confirming "did this thread
      already get an answer, and does GitHub still consider it current" is a
      glance at the comment you're already looking at, on every manual refresh
      (`r`), regardless of whether the reply came from `R`, `n`, or an agent's
      own `gh` call. Unit-tested (`replies_in` finds only same-thread replies;
      `reply_posted_via_amf` on/off the footer; detail-pane render tests for no
      section when there are no replies, the via-AMF chip appearing/not
      appearing, and the outdated/resolved chips showing next to the reply). →
      `.claude/commands/amf/pr-continue.md`, `src/app/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`.

- [x] **Open PR Triage work in a new AMF feature with independently chosen
      settings.** The current dedicated triage session may use a different
      harness, but it still inherits the source feature's vibe mode and launch
      flags. Add a `New feature…` fix-target option alongside the existing-live
      and same-feature dedicated-session targets. Before the first fix, open a
      compact feature-creation flow that can use a configured feature preset or
      manually choose the harness, vibe mode, and other relevant feature
      settings. This must support workflows such as implementing the original
      feature in SuperVibe mode and doing review triage in a new Vibeless
      feature.

      Create an isolated worktree/tmux-backed AMF feature so worktree-local
      hooks and permissions for the triage feature do not mutate the source
      feature. Seed it from the PR head and retain an explicit link to the
      source PR and source feature, because Git cannot check out the same branch
      in two worktrees and branch-based PR auto-detection will not be sufficient
      for the companion branch. Define a safe, visible integration path for
      triage commits (for example, an explicit push to the PR head ref or a
      guided cherry-pick back into the source feature); never silently overwrite
      or diverge a dirty source worktree. Reuse the new feature for every fix in
      that PR, preserve the existing return-to-triage navigation/status, and
      show the selected feature and mode in the fix confirmation UI. Acceptance:
      start from a SuperVibe PR feature, choose `New feature…` + Vibeless, inject
      and complete multiple fixes in the isolated feature, land them on the
      original PR branch through the confirmed integration path, and return to
      the same PR Triage state.

      **Shipped.** A third `FixTarget::NewFeature` sits alongside
      `ExistingLive`/`DedicatedReview`, added as a last row
      (`FixTargetPickRow::NewFeature`) in the same first-fix target picker — last
      deliberately, since it costs a worktree and an explicit integration step,
      so it isn't what the cursor lands on. Choosing it opens
      `TriageFeatureSetupState`, a **single settings list** rather than a re-run
      of the multi-step feature wizard (the user is mid-triage, and the wizard's
      source-worktree / existing-worktree / session-name / task-prompt steps all
      have an obvious answer here): Preset · Harness · Vibe mode · Review mode ·
      Chrome · Branch. `j/k` move, `h/l` change, `⏎` creates, `esc` abandons the
      fix *and* leaves the target unresolved so the next `f` re-offers every
      option. Selecting a configured feature preset fills the rows beneath it —
      including applying its `branch_prefix` to the companion branch — and leaves
      them editable, so a preset is a starting point rather than a lock. Plan mode
      is deliberately absent: it defers the launch into a planning interview,
      which makes no sense for a feature whose job is to apply comments that
      already say what to do.

      **Isolation and the link.** `create_triage_feature`
      (`src/app/triage_feature.rs`) resolves a base via `triage_base` — the source
      feature's local branch when it already *contains* the PR head (so unpushed
      commits aren't silently dropped), otherwise the head SHA itself, fetched
      first if absent — and builds a worktree with
      `WorktreeManager::create_from`. The companion branch defaults to
      `<pr-branch>-triage`, de-duplicated (`-2`, `-3`, …) against both existing
      feature names and existing git branches, because **git can't check the PR's
      own branch out in two worktrees**. Its own worktree is exactly what keeps
      the triage agent's hooks and permissions off the source feature; the
      `on_worktree_created` hook runs synchronously (the interactive prompt flow
      would have to unwind the PR pane) and a failure warns rather than aborting.
      The feature's primary session carries the existing `"PR Triage"` label, so
      `pr_triage_session_index` finds it inside the companion exactly as it does
      the in-feature dedicated session. Since branch-based auto-detection can't
      get back, the link is persisted explicitly as `Feature::triage_source`
      (`TriageSource { pr_number, source_feature_id, pr_branch, base_sha }` —
      `pr_branch` is the PR's own `head_ref`, which is what integration pushes
      onto, and is not necessarily the source feature's checked-out branch,
      JSON blob column added by migration 015, `#[serde(default)]` so pre-existing
      rows/JSON still load).

      **Reuse and redirection.** A new `App::pr_review_target_feature` /
      `pr_review_feature_for_target` resolves the *companion* feature for this
      target and the source feature otherwise; `resolve_fix_session`,
      `pr_review_fix_session_usage`, `pr_review_dedicated_session_working`, and
      the `P` toggle (`pr_review_toggle_to_session`) all route through it, so
      injection, the header's usage/working badges, and the return-to-triage
      round trip follow the fix to whichever feature it lands in.
      `adopt_existing_triage_feature`, called on both pane-entry paths (cache hit
      and background fetch), finds an existing companion by the persisted link and
      marks the target resolved — so **every fix in the PR reuses the same
      feature** across pane re-opens and restarts without re-asking, mirroring the
      existing "a dedicated session already exists, don't re-offer the picker"
      rule one level up. The fix confirm dialog and the pane header both name the
      companion as `<feature> · <harness> · <mode>`
      (`pr_review_triage_feature_summary`), since "which feature and which mode"
      is the entire point of the target.

      **Integration.** `I` opens `TriageIntegrateState`: the companion branch →
      PR branch, the commits on the companion since `base_sha`, and two explicit
      options. **Push** runs `git push origin <triage>:<pr-branch>` — never
      `--force`; a diverged PR branch is reported with a "pull it in and try
      again" message instead of being overwritten. **Cherry-pick** applies
      `base_sha..<triage>` into the source worktree and is refused outright while
      that worktree is dirty (checked when the overlay opens — the row renders
      `[unavailable]` with the reason — *and* again inside `cherry_pick_range`, so
      no future caller can bypass it); a conflicting pick is aborted so the source
      worktree comes back exactly as it was found. Uncommitted work in the
      *triage* worktree is called out too, since it wouldn't be included.
      Successes and failures both stay in the overlay so a rejected push can be
      read and retried in place.

      Unit-tested: the picker's row order, `New feature…` opening setup instead of
      injecting, branch pre-fill/de-duplication, row cycling and wrapping, the
      branch text row, preset application (including its branch prefix), cancel
      leaving the target unresolved, companion-vs-source session resolution (with
      a decoy same-labelled session in the source feature), adopt-on-entry and its
      PR-number scoping, the confirm-dialog summary, `I` rejected for the
      in-feature targets, and — against real git repos — `triage_base` preference,
      `commits_since`, dirty detection, a successful cherry-pick, a refused one,
      an empty range, and a conflicting pick leaving the source clean. Plus
      persistence: SQLite round trip of `triage_source` and JSON round trip /
      absent-field back-compat. Full suite green (1297 passed, 1 ignored); strict
      clippy and formatting clean. →
      `src/app/triage_feature.rs` (new), `src/app/pr_review.rs`,
      `src/app/state.rs`, `src/app/mod.rs`, `src/app/session_ops.rs`,
      `src/app/automation.rs`, `src/app/review.rs`, `src/project.rs`,
      `src/db/migrations.rs`, `src/db/store.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`, `src/ui/dialogs/diff.rs`,
      `src/ui/dialogs/help.rs`, `src/ui/dashboard.rs`, `src/app/tests.rs`,
      `README.md`.

- [x] **Carry an agent-written reply draft from fix injection into the Reply
      flow.** Extend the prompt injected by `f` (and each entry in `B`'s
      combined prompt) so the agent does two things in order: make and verify
      the requested change, then write a concise reviewer-facing reply that
      explains what it changed. The agent must hand that text back to AMF as a
      draft, not post it to GitHub itself. Capture the latest non-empty draft
      per `PR# + comment id` through a provider-neutral, machine-readable
      handoff (do not scrape free-form terminal output), and persist it in the
      local triage layer so it survives the round trip from the fix session
      back to PR Triage. Starting a new fix for the same comment should replace
      or invalidate the older draft; batch replies must stay correlated with
      their individual comment ids.

      When the user presses `R` and chooses a reply kind, seed the editable
      reply dialog with that comment's captured agent draft when one exists.
      If no draft exists, preserve the current seed for that reply kind:
      `Done` still uses `commit_for_done_reply` (`Done in \`<sha>\`.` with its
      existing fallback caveat), while `Not needed` still opens with its
      current empty seed. This only changes autofill: the user still reviews
      and edits the text, explicitly confirms the GitHub write, and separately
      chooses whether to resolve the thread. Acceptance: inject a fix, let the
      agent complete it and supply a draft, return with `leader+P`, press `R`,
      and see that draft prefilled; repeat without an agent draft and see the
      existing reply seed unchanged.

      **Shipped, 2026-07-20.** Every single or combined fix-confirm prompt now
      appends one correlated handoff command per comment. After making and
      validating the change, any built-in harness can pass its proposed reply
      on stdin to the hidden `amf reply-draft` command; that command sends
      structured IPC to AMF, so the TUI never scrapes agent terminal prose.
      `ReplyDraftRequest` gives each comment a fresh UUID when the dialog is
      built and records the PR head that existed before the fix. Confirming the
      injection activates those ids in the new `pr_comment_reply_drafts`
      SQLite table (migrations 013–014), clearing any older body. IPC updates
      only the currently active id, so a late agent response from an earlier
      fix cannot overwrite the latest draft. Drafts age out with the rest of
      local triage state.

      `R` now loads the selected comment's captured draft before opening either
      reply kind. A captured draft opens post-ready in the existing editable
      confirm view. Done drafts append `Done in <sha>` only when AMF finds a
      commit after the recorded pre-fix head that touched the comment's file;
      this catches adjacent insertions without mislabeling the older commit
      that originally introduced an unchanged commented line. The handoff
      prompt tells the agent not to guess a hash.
      Without a draft, Done still uses `commit_for_done_reply` and Not needed
      still opens empty in edit mode. Successful posting consumes the stored
      draft and uses the accurate `— drafted by AI via AMF` footer, while
      non-agent templates retain `— posted via AMF`; both still require the
      user's explicit GitHub-write confirmation. Tests cover the CLI contract,
      single/batch prompt correlation, request replacement and stale-response
      rejection, IPC persistence, injection activation, draft preference for
      both reply kinds, commit-reference composition, attribution selection,
      and unchanged no-draft fallbacks. Full suite green (1184 passed, 1
      ignored); strict clippy and formatting clean. →
      `src/main.rs`, `src/app/notifications.rs`, `src/app/pr_review.rs`,
      `src/app/state.rs`, `src/db/migrations.rs`,
      `src/db/pr_comment_triage.rs`, `src/db/mod.rs`, `src/app/tests.rs`,
      `src/ui/dialogs/pr_review.rs`, `CHANGELOG.md`.

- [x] **Surface a completed, pending AI Review in PR Triage and refresh
      Triage automatically after posting it.** Splitting AI Review into its
      own pane restored an in-progress `[AI review running]` badge in PR
      Triage, but the signal disappears when generation completes. If the
      current PR and head SHA have a completed AI Review with one or more
      publishable findings, PR Triage must show an obvious pending-review
      badge (including the finding count) until those findings are posted,
      skipped, invalidated by a new head SHA, or otherwise no longer
      publishable. The indicator must be backed by the persisted
      `ai_review_cache`, not only in-memory background-job state, so it also
      appears after leaving the pane or restarting AMF. It should be
      actionable through PR Triage's existing `A` entry point, reopening the
      matching AI Review rather than starting a duplicate run. Do not show a
      pending badge for a zero-finding, failed, fully skipped, or fully
      published review, or for another PR/worktree.

      The generation pass must also produce and persist a short overall
      summary of the full review (one to three useful sentences covering the
      main themes or risk), and `W` must use that as the GitHub review body
      instead of the fixed `AI review, via AMF.` placeholder. Show the summary
      in the existing editable post-confirm dialog so the user can revise it
      before the GitHub write, and retain clear AI-via-AMF attribution without
      making the attribution itself the summary. Generate it in the same
      agent pass as the findings rather than spending a second review call;
      persist it beside the findings so reopening a pending review does not
      regenerate it. Older cached reviews or malformed output that lack a
      summary may fall back to the current placeholder rather than blocking
      posting.

      After `W` successfully posts an AI Review to GitHub, automatically run
      PR Triage's normal network refresh for that PR (bypassing its cached
      comment blob) so the newly created review comments and thread state are
      available without requiring a manual `r`. If AI Review was opened from
      PR Triage, refresh the stashed pane while preserving its local triage
      state and return navigation; if it was opened elsewhere, invalidate or
      update the matching PR Triage cache so the next entry is fresh. Start
      this only after GitHub confirms the post, never after a failed/cancelled
      post, and keep the posted/pending marker durable before beginning the
      fetch so a refresh failure cannot make `W` post the same review twice.
      Acceptance: generate an AI Review, leave it unposted, and see a pending
      count in PR Triage (including after restart); reopen it from Triage,
      post with `W`, return to Triage, and see the pending badge clear and the
      posted GitHub comments appear without pressing `r`.

      **Shipped, 2026-07-22.** PR Triage now loads an exact publishable-finding
      count from the persisted `ai_review_cache` for the current PR/head SHA
      and renders `[AI review pending: N]` whenever no run is in progress. The
      count follows generation, skip/unskip, publication, pane stashes, and
      restarts; zero-finding, failed, fully skipped/published, other-PR, and
      stale-head results do not surface. `A` continues through the shared
      cached AI Review entry point, so the badge opens the matching findings
      without regenerating them.

      The same headless pass now emits a one-to-three sentence summary before
      its findings. AMF parses and persists that summary beside the findings,
      seeds the existing editable `W` confirmation with it, and adds separate
      AI-via-AMF attribution to the final GitHub body. Summary-only clean
      reviews no longer look like parse failures; older/malformed cached
      output without a summary retains the legacy placeholder.

      A successful GitHub write durably marks every included finding published
      before starting a separate background PR Triage refresh. The refresh
      bypasses and invalidates the old comment cache, preserves the stashed
      pane's selection, filters, marks, fix-target state, and return navigation,
      and caches the fresh snapshot even when AI Review was opened elsewhere.
      Failures leave the published marker intact and surface a warning instead
      of making the review postable again. Regression coverage includes summary
      parsing/fallback and cache compatibility, durable publishable counts,
      the pending badge, cached `A` reopen, editable `W` body, stashed-pane
      refresh, cache-only refresh, and precise cache invalidation. Full suite
      green (1209 passed, 1 ignored); strict Clippy and formatting clean. →
      `src/app/ai_review.rs`, `src/app/pr_review.rs`, `src/app/state.rs`,
      `src/app/mod.rs`, `src/main.rs`, `src/db/ai_review_cache.rs`,
      `src/db/pr_review_cache.rs`, `src/db/mod.rs`, `src/ui/dashboard.rs`,
      `src/ui/dialogs/ai_review.rs`, `src/ui/dialogs/pr_review.rs`,
      `src/app/tests.rs`, `CHANGELOG.md`.

- [x] **BUG — the AI Review model picker cannot go back to change the
      harness.** In the `A` generation flow, choosing Claude/Codex/Opencode/etc.
      immediately advances to the model picker. `Esc` from that model list
      currently closes only the model picker while leaving the harness locked
      in, so a mistaken harness choice cannot be corrected without abandoning
      and reopening the pane. Make the two pickers behave like adjacent wizard
      steps: `Esc`/`q` from the model **list** returns to the harness picker,
      with the current harness highlighted; `Esc` while editing a custom model
      keeps its existing behavior of returning to the model list first. Picking
      a different harness must rebuild the available model rows for that
      harness and must not retain an incompatible model choice. Keep Pi's
      intentional model-picker bypass. Add App/key-handler regression coverage
      for custom editor → model list → harness list, changing the harness, and
      continuing through the rebuilt model list.

      **Shipped, 2026-07-23.** The model list's `Esc`/`q` path now restores the
      harness picker with the current harness highlighted, while `Esc` from
      custom-model editing still returns to the model list first. Confirming a
      harness clears the previous harness's model state and rebuilds the rows;
      Pi still follows its direct default-model path. The dialog hints now call
      out the harness back-step. App/key-handler regression coverage exercises
      custom editor → model list → harness list, switches from Claude to
      Opencode, and verifies the rebuilt picker contains no Claude-only preset.
      An isolated seven-frame walkthrough is recorded under
      `docs/screenshots/ai-review-picker-back-navigation/`. AI Review tests,
      strict Clippy, and formatting are green. → `src/app/ai_review.rs`,
      `src/app/state.rs`, `src/ui/dialogs/ai_review.rs`, `src/app/tests.rs`,
      `docs/screenshots/ai-review-picker-back-navigation/`, `CHANGELOG.md`.

- [x] **Hide or de-emphasize AMF's own follow-up replies in the PR Triage
      list.** Replies posted by the `R` flow already end with the exact
      `— posted via AMF` footer, and AI Review findings posted by AMF carry the
      corresponding `— drafted by AI via AMF` footer. Use these durable markers
      together with the reply's parent relationship to distinguish follow-up
      history from standalone findings after a refresh. An inline
      `Done in \`<sha>\`.`/not-needed reply should remain visible under its root
      comment's existing **Replies** section, but should not also appear as a
      normal actionable row, inflate the open-comment count, or be offered back
      to the agent as another fix.
      For orphaned AMF follow-up replies whose parent row was not fetched,
      retain access in a muted/gray presentation rather than silently
      discarding them. Standalone comments, including AI Review findings posted
      through AMF, remain normal actionable work. Match only AMF's exact reply
      attribution footers so ordinary comments that merely mention AMF are not
      hidden. Add normalization/filter/count and render coverage, including a
      refresh after posting through `R`, an AI Review-authored comment, and an
      unrelated human reply that must remain visible and actionable.

      **Shipped, 2026-07-24.** Exact durable attribution footers distinguish
      follow-up replies posted through PR Triage from standalone findings. A
      refreshed attributed inline reply remains in the normalized/cache model
      and in its root comment's **Replies** section, but
      `PrReviewState::visible_indices` collates away the duplicate list row when
      that root is present. An orphaned follow-up remains visible with a muted
      `[via AMF]` context-only treatment. Standalone findings — including those
      posted by AI Review — retain the normal open count, fix, batch, reply,
      mark, and memory actions. Exact-marker matching leaves ordinary human
      replies that merely mention AMF visible and actionable. Normalization,
      refresh-shaped classification, orphan/filter/count, fix/batch, and
      list/detail render regressions cover the complete flow. Full suite green
      (1234 passed, 1 ignored); strict Clippy and formatting clean. →
      `src/app/ai_review.rs`,
      `src/app/pr_review.rs`, `src/app/state.rs`, `src/app/tests.rs`,
      `src/ui/dialogs/pr_review.rs`,
      `docs/screenshots/pr-triage-amf-outbound-comments/`, `CHANGELOG.md`.

- [x] **Show in-progress AI Review generation on the dashboard's PR item.**
      While an AI Review is generating for a feature, append an `AI review`
      activity marker to that feature's existing `[PR #N · M open]` badge in
      the dashboard list. Reuse the workdir-scoped
      `ai_review_running_for_workdir` state that drives the in-session PR
      badge, so the marker appears only on the feature whose review is running,
      remains visible if the user leaves the AI Review pane, and clears when
      generation succeeds, fails, or is cancelled. Keep the existing PR number
      and unresolved-thread count readable, and add render coverage for the
      running, unrelated-feature, and completed states. →
      `src/ui/list.rs`, `src/app/ai_review.rs`.

      **Shipped, 2026-07-25.** The dashboard feature row's PR badge now reads
      `[PR #N · M open · AI review]` while a background review runs, mirroring
      the in-session `pr_triage_badge_span` marker and switching the badge to
      the warning color, with the PR number and unresolved count left intact.
      It reads the same workdir-scoped `ai_review_running_for_workdir` state
      the pane badge uses, so it survives leaving the AI Review pane and is
      scoped to the one feature whose review is running. `src/app/ai_review.rs`
      needed no change — the existing accessor was already sufficient. Render
      regressions cover the running, unrelated-feature, and completed states,
      the last one asserting the marker is gone at the intermediate step where
      `poll_ai_pr_review_bg` has cleared the background slot but not yet taken
      the pending snapshot. Suite green (1263 passed, 1 ignored; the one
      failure is the pre-existing `wsl_clipboard_round_trips_image_and_text`
      test, which needs `wl-paste`/`xclip` and is unrelated); strict Clippy and
      formatting clean. → `src/ui/list.rs`, `CHANGELOG.md`.

## Reasoning / when to build

Build after the prompt-library injection seam is stable (Epic B depends
on it). Epic A is independently valuable and low-risk (read-only, no
agent, no writes) — a good first slice to ship and validate the fetch /
normalize / strip pipeline before wiring in any agent or GitHub writes.
The token discipline (metadata-first, minimal fix prompts, warm-session
reuse, caching) is what makes high-volume review loops affordable and is
the reason to do this in AMF rather than in the agent chat directly.
