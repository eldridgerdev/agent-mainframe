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
- [ ] **Syntax-installer `i` shortcut in the review pane (parity with the diff
      viewer).** The detail pane now syntax-highlights the diff hunk via the
      shared tree-sitter highlighter, but highlighting silently degrades to
      plain marker coloring when the parser for the hunk's language isn't
      installed. The diff viewer (and the diff-review prompt) already expose an
      `i` key that opens the syntax-language picker for the *selected file's*
      language so the user can install/uninstall the parser without leaving the
      pane (`handlers/diff.rs` → `open_syntax_language_picker_for_selected_diff_file`
      in `src/app/syntax.rs`, returning to the originating mode via the picker's
      `return_to`). Bring the same affordance to the PR-review pane:
      - Add `KeyCode::Char('i')` to `handle_pr_review_key`
        (`src/handlers/pr_review.rs`) that opens the picker for the **selected
        comment's** `path` (skip/no-op for conversation/summary comments with no
        file path).
      - Extend `open_syntax_language_picker_for_selected_diff_file` (or factor a
        shared helper) with an `AppMode::PrReview` arm that pulls the selected
        comment's path, computes the `syntax_notice_for_path` hint, and stashes
        the current `PrReviewState` as the picker's `return_to` so closing the
        picker drops the user back into the same pane and selection.
      - The picker already polls background install/uninstall ops and calls
        `crate::highlight::reload_runtime_state()` (which clears the highlight
        cache) on completion; since the review detail re-highlights every draw,
        the hunk should pick up the freshly installed parser automatically on
        return — verify that and clear any per-pane cache if added later.
      - Surface discoverability: add `i syntax` to the pane footer key hints and
        the keybinding help entry, and consider a small inline hint in the diff
        section when a language is detected but its parser isn't installed (e.g.
        `Rust highlighting not installed — press i`), reusing
        `HighlightLanguage::install_state` / `language_install_state_for_path`.
      → `src/handlers/pr_review.rs`, `src/app/syntax.rs`, `src/app/state.rs`,
      `src/ui/dialogs/pr_review.rs`.
- [ ] **PR picker — list PRs to choose from, or enter a number.** Today the
      entry point auto-detects the branch's PR and, on a miss, drops straight to
      the manual PR-number prompt (`AppMode::PrNumberPrompt`). Add a third path:
      a selectable list of the repo's PRs so the user can pick one without
      knowing its number. Fetch via `gh pr list --json
      number,title,author,headRefName,updatedAt,isDraft,state` (in Rust, zero
      agent tokens, reuse the `GhCli` layer), show a scrollable picker
      (number · title · author · branch, newest first), and on select run the
      existing resolve → load path. The manual number prompt stays available
      from the picker (e.g. a key or a "enter a number instead" affordance) so
      both flows live behind one entry: **search/pick a PR _or_ type its
      number**. Default the list to open PRs with a toggle to include
      closed/merged; consider seeding the highlight on the branch's
      auto-detected PR when there is one. → new picker mode + handler, peer to
      `PrNumberPrompt`; `gh pr list` wrapper in `src/github.rs`.
- [ ] **Pick the agent harness before the dedicated review session starts.**
      Today the dedicated review session is spun up with the project's
      `preferred_agent` (`create_dedicated_review_session` in
      `src/app/session_ops.rs` reads `projects[pi].preferred_agent` →
      `session_kind_for_agent`). Let the user choose the harness (Claude /
      Codex / opencode, i.e. `AgentKind`) for the review session **before** the
      first fix is injected, so PR triage can run on a different harness than
      the feature's working session (e.g. a cheaper/faster model for
      mechanical review fixes). Surface a harness picker on the first `f` for
      the `DedicatedReview` target (reuse the existing harness-selection UI —
      `src/ui/dialogs/harness.rs` — rather than a bespoke menu), remember the
      choice for the rest of the PR (the session is created once and reused),
      and fall back to `preferred_agent` as the default highlight. Only applies
      to the dedicated target; `ExistingLive` reuses whatever harness that
      session already runs. → `src/app/pr_review.rs`,
      `src/app/session_ops.rs`, `src/app/state.rs`, plus the harness picker.
- [ ] **Upgrade the fix confirm/edit dialog editor (vim + full editing).**
      The dialog added in Epic B seeds a plain `TextEditor::new(prompt)` and
      forwards keys to it only in edit mode — enough to tweak the prompt, but
      it lacks the niceties the other editor-backed dialogs already have.
      Bring it up to parity: **vim keymap support** (`TextEditor::with_vim` /
      the `toggle_vim` toggle, with a persisted `vim_enabled` choice like
      `PlaceholderFillState`), a visible mode/cursor indicator, scroll +
      cursor-follow for prompts taller than the dialog (reuse
      `editor_view::sync_editor_scroll` / `editor_cursor_visual_row` instead of
      the current non-scrolling `Paragraph`), and the standard editing affordances
      (undo/redo, word motions — already in `TextEditor`, just not surfaced
      here). Decide a submit gesture that coexists with multi-line editing
      (e.g. `Ctrl-Enter` to inject vs. `Enter` for a newline) since today
      `Enter` only injects from the confirm view, not edit mode. → `src/app/state.rs`
      (`FixConfirmState`), `src/app/pr_review.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`.
- [ ] **Fix several comments in one pass against the same dedicated session.**
      Per-comment reuse already works (`fix_session_index` finds the dedicated
      `"PR Review"` session by label, so every `f` reuses the same warm
      harness). What's missing is the *throughput loop*: today each `f`
      switches the user into the session to watch, so fixing N comments means N
      round-trips out of and back into the pane. Let the user **select
      multiple comments** (multi-select / a `[space]`-marked set) and inject
      their fix prompts into the same dedicated session **in sequence without
      leaving the pane** between each — the agent works through them while the
      user keeps triaging, and the per-session overhead and warm file context
      are shared across all of them (token principle #4). Each comment stays a
      **separate** fix prompt here (sequential), which is what distinguishes
      this from the batch item below (one combined numbered prompt). Mark each
      as `Fixing` as it's queued. → `src/app/pr_review.rs`,
      `src/handlers/pr_review.rs`, `src/ui/dialogs/pr_review.rs`.
- [ ] **Combined-prompt batch: "fix all of these, then I'll come back."**
      The walk-away workflow. Let the user mark a set of comments to fix
      (reuse the same multi-select from the sequential item above), then build
      **one numbered prompt** that lists every selected comment — each with its
      `file:line` pointer, bot-stripped text, and diff hunk (still no file
      contents) — and inject it **once** into the dedicated review session so
      the agent works through the whole list autonomously while the user is
      away. Differs from the sequential item above in that it's a single
      combined prompt (shared preamble + file context across all comments → the
      cheapest token path for a big set), and it's send-and-leave rather than
      watch-each. Show the same confirm/edit dialog with a `~N tokens` preview
      for the assembled batch before sending, mark every included comment
      `Fixing` on send, and on the next refresh reconcile which threads got
      resolved/answered. Keep the set bounded (warn past some comment/token
      ceiling) so a single prompt doesn't blow the context window. →
      `src/app/pr_review.rs`, `src/handlers/pr_review.rs`,
      `src/ui/dialogs/pr_review.rs`.
- [ ] Filters/sort (open-only, by file, by author, humans-first).
- [x] "Done in `<sha>`" reply template auto-filled from latest commit. Shipped
      with the Epic C reply work: `R` seeds a reply with the feature workdir's
      short `HEAD` (falling back to "Done." outside a git repo), editable before
      posting; on post the comment is marked `Done`. → `src/app/pr_review.rs`.
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
