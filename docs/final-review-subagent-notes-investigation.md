# Final Review: offloading review-notes writing to subagents

**Status:** investigation only. This document evaluates *whether and how* the
primary feature agent can stop paying to repeatedly read and rewrite Final
Review's notes file — by handing the writing to a subagent / orchestrated
one-shot, **or** by changing what the agent is asked to do so the expensive
part stops happening. Implementation is deferred to a separate follow-up
feature; this doc ends with a go/no-go recommendation. (The recommendation, §7,
lands on the second approach — a one-line instruction change — not a subagent.)

The approved plan for this investigation is `AMF_PLAN.md` in this worktree.

**Review status:** draft complete, awaiting maintainer sign-off. Every code
reference below was cross-checked against the tree at branch
`final-review-investigate-using`. The recommendation in §7 and the go/no-go in
§8 are the decision points that need maintainer review before a follow-up
feature is opened.

**Implementation status:** Step 1 (Option F — blind-append, never read
`.claude/review-notes.md`) has been implemented in `ensure_review_claude_md`
(`src/app/setup.rs`), with P0 covered by
`blind_appended_duplicates_collapse_below_the_cap` /
`archive_review_notes_collapses_blind_appended_duplicates_on_disk`
(`src/app/review.rs`). Options I and K, and Step 2 (the AMF-driven mechanical
writer + opt-in headless enrichment), remain deferred pending dogfood results.

## Contents

1. [Current final-review flow](#1-current-final-review-flow)
2. [Cost baseline](#2-cost-baseline)
3. [Orchestration options for subagents](#3-orchestration-options-for-subagents)
4. [Review-files spec options](#4-review-files-spec-options)
5. [Harness support matrix](#5-harness-support-matrix)
6. [Prompt-registry integration](#6-prompt-registry-integration)
7. [Comparison and recommendation](#7-comparison-and-recommendation)
8. [Follow-up go/no-go](#8-follow-up-gono-go)

---

## 1. Current final-review flow

### 1.1 Two different files, routinely conflated

AMF's Final Review touches two independent Markdown files under the feature
worktree's `.claude/` directory. They are easy to mix up because both live in
`.claude/`, both are gitignored, and both have an `-archive.md` sibling. Only
the **first** is what the plan's cost premise is about.

| File | Author | Reader | Purpose |
| --- | --- | --- | --- |
| `.claude/review-notes.md` (+ `review-notes-archive.md`) | the **feature agent** (while Review Mode is on) | AMF (shows it beside the diff); the agent re-reads it each batch to avoid duplicate sections | Per-changed-file "what changed and why" developer notes, captured *as the work happens* |
| `.claude/final-review-feedback.md` (+ `final-review-feedback-archive.md`) | **AMF itself** (`complete_final_review`), never an agent | the feature agent, once per round, via `REVIEW_FEEDBACK_PROMPT` | The human reviewer's verdicts / line comments / suggestions for a finished review round |

Two more `.claude/` files support an in-flight review but carry no agent cost:

- `.claude/final-review-progress.json` — resumable in-flight review state
  (`review_progress_path`, `src/app/review.rs:126`). Written after every
  state-changing review action (`persist_review_progress`,
  `src/app/review.rs:256`); **cleared on finish** (`clear_review_progress`,
  `src/app/review.rs:373`).
- `.claude/final-review-snapshot.json` — a diff fingerprint of the last
  *finished* round for "what changed since the reviewer last looked" detection
  (`review_snapshot_path`, `src/app/review.rs:175`; `save_review_snapshot`,
  `src/app/review.rs:382`). **Not** cleared on finish.

All five paths are added to `.claude/.gitignore` by `ensure_notification_hooks`
(`src/app/setup.rs:1238-1243`) and again by `ensure_review_claude_md`
(`src/app/setup.rs:1394-1395`). Nothing here is ever committed.

### 1.2 The cost driver: Review Mode's `review-notes.md` loop

**Where it is turned on.** A feature carries a `review: bool` flag
(`src/project.rs:362`). When true, `ensure_review_claude_md(workdir, enabled)`
(`src/app/setup.rs:1352`) is invoked on essentially every start/attach path:
`feature_ops.rs:868`, `session_ops.rs:1118` and `:1246`,
`claude_session_picker.rs:242`, `codex_session_picker.rs:244`,
`opencode.rs:249`.

**What it writes.** `ensure_review_claude_md` injects an AMF-managed block into
the worktree's `CLAUDE.local.md` (Claude Code's gitignored, auto-read variant of
`CLAUDE.md`), delimited by `<!-- AMF:review-instructions:begin -->` /
`<!-- ...:end -->`. The block (`src/app/setup.rs:1355-1378`) instructs the
agent, verbatim in intent:

> When you finish a **logical batch** of related changes (**not** after every
> individual Edit/Write — batch it), append one section per file you touched in
> that batch to `.claude/review-notes.md`. Keep each note to 1-2 sentences: what
> changed and why. Skip a file if it already has a note from an earlier batch
> and there is nothing new to add. […] Only inspect the bounded live file when
> deciding what to append; the archive is reviewer history and does not need to
> be read.

Section format is fixed: `## <relative-file-path> — <brief title>` then a 1-2
sentence body then a `---` rule.

> **Harness reach — important.** `ensure_review_claude_md` writes **only**
> `CLAUDE.local.md`, unconditionally, with no harness parameter — unlike
> `ensure_plan_mode_instructions` (`src/app/setup.rs:1249`+), which *is*
> harness-aware and writes `AGENTS.md` for non-Claude agents. `CLAUDE.local.md`
> is Claude Code's auto-read file; Codex (`AGENTS.md`), OpenCode
> (`AGENTS.md` / `CLAUDE.md`), and Pi (its own context files) do **not**
> reliably read it. So the `review-notes.md` loop — and its cost — is
> **effectively Claude-only today**. This matters twice below: the §2 baseline
> is a Claude-session cost, and any option that keeps working through this
> instruction (Family 3) inherits the same Claude-only reach until
> `ensure_review_claude_md` is made harness-aware (a small, orthogonal
> prerequisite — see §5, §8.3).

> **Doc drift note:** `docs/backlog/token-efficiency-plan.md` §7 (and its line
> 40) describes this as "a prose note **before every** Edit or Write". That is
> stale — the shipped block explicitly batches per logical group. The per-turn
> tax is real but is *per batch*, not *per edit*. The baseline in §2 must model
> the shipped behaviour, not the plan's description of it.

**How the agent reads and rewrites it.** Because the instruction says only the
"bounded live file" need be inspected, each batch the agent:

1. reads `.claude/review-notes.md` (to see which files already have a section
   and what they say), then
2. writes its new/updated sections back — an append in the common case, an edit
   when refining an existing file's note.

The live file is bounded to the **50** most-recently-documented files
(`MAX_LIVE_REVIEW_NOTE_FILES`, `src/app/review.rs:69`). AMF keeps it bounded
*without the agent's help*: `archive_review_notes_after_agent_turn`
(`src/app/notifications.rs:367`) fires on every `thinking-stop` / `stop` /
`input-request` IPC notification for a `feature.review` feature and calls
`archive_review_notes(workdir)` (`src/app/review.rs:5153`). That function:

- reads the live file; `split_overflow_review_notes`
  (`src/app/review.rs:5121`) keeps only the newest section per path and only
  the `keep` most-recent paths, routing the rest to overflow;
- writes the archive **first** (`write_review_notes_atomic`, a temp-file +
  `sync_all` + `persist` replace, `src/app/review.rs:5095`), then the trimmed
  live file, so a failed archive write never loses history.

`archive_review_notes` is also run once at setup time
(`ensure_review_claude_md` → `src/app/setup.rs:1396`).

**How AMF reads it back.** `load_review_notes(workdir)`
(`src/app/review.rs:5196`) parses the archive then the live file (live wins on
key collision) into a `path -> note` map via `parse_review_notes`
(`src/app/review.rs:5213`). During a review this populates
`DiffViewerState::review_notes` (`src/app/state.rs:957-959`), refreshed at
`src/app/diff.rs:158`, and is shown in the diff viewer's developer-notes panel.
For a changed file with **no** note, the reviewer can press `g` to generate one
on demand with a headless Claude call (the `review.walkthrough` prompt), cached
in `DiffViewerState::generated_notes` (`src/app/state.rs:961`; generator around
`src/app/review.rs:1595`).

### 1.3 How Final Review is launched

- Key **`f`** in an embedded session view → `handlers/view.rs:397` →
  `App::trigger_final_review()` (`src/app/review.rs:225`).
- It requires `AppMode::Viewing`, resolves the feature `workdir` by project +
  feature name, constructs `DiffViewerState::new(view, workdir)`, sets
  `state.review = true` and `state.layout = preferred_diff_viewer_layout()`, and
  enters `AppMode::DiffViewerLoading(state)`.
- **No agent, no tmux session, no headless call** is part of launching the
  review. Final Review is AMF's native TUI diff viewer walking every file
  changed against the base ref, with the reviewer approving / rejecting /
  commenting per file. Saved `final-review-progress.json` is reloaded on open
  (the restoration choke point is documented at `src/app/review.rs:456`).

### 1.4 Finishing a review and consuming its output

`finish_final_review()` (`src/app/review.rs:3120`) optionally runs the project's
`final_review_check_command` build/test gate in the background
(`poll_final_review_check`, `src/app/review.rs:3195`, driven from
`src/main.rs:1242`), then calls `complete_final_review(check)`
(`src/app/review.rs:3273`):

1. `clear_review_progress(workdir)`; `save_review_snapshot(...)` for the
   just-reviewed diff (even an all-approved round).
2. Tally approve / reject / skip; collect open line-comment threads, open
   file-comment threads, and general feedback.
3. **No actionable feedback** → persist an "all approved" round to history for
   the review timeline, return to the session view, dispatch nothing to any
   agent.
4. **Actionable feedback** → build one self-contained Markdown *round* section:
   a `## Review — <ts>` preamble with counts (`review_round_preamble`,
   `src/app/review.rs:4673`), then `### General Feedback`, `### Files Needing
   Revision` (each `#### <path> — [severity]`), `### File Comments`, `### Line
   Comments` (with `[severity]` tags and verbatim ` ```suggestion ` blocks).
5. `persist_final_review_round(workdir, round)` (`src/app/review.rs:3240`) →
   `compose_feedback_log(existing, round)` (`src/app/review.rs:4650`)
   **prepends** the new round under a single `FEEDBACK_TITLE`, so
   `.claude/final-review-feedback.md` is an append-only trail. Rounds past
   `MAX_LIVE_ROUNDS = 2` (`src/app/review.rs:4736`) are moved to
   `.claude/final-review-feedback-archive.md`; only the newest round is ever
   consumed.
6. Optionally mirror the round onto the branch's GitHub PR as a review
   (`post_final_review_to_pr`, `src/app/review.rs:3838`) when
   `final_review_post_to_pr` is set. The local file is the source of truth
   regardless.
7. `dispatch_review_feedback(from_view, summary, fix_target,
   fix_target_feature_id, review_harness)` (`src/app/review.rs:3590`).

### 1.5 `review_destination` — where the fixes are dispatched

The reviewer chooses a destination in the "dispatch fixes to…" picker
(`src/app/review_destination.rs`, `src/handlers/review_destination.rs`). Rows
`ExistingLive` / `Dedicated` / `ExistingFeature` / `NewFeature`
(`ReviewDestinationRow`, `src/app/review_destination.rs:114-122`) each map to a
`FixTarget` (`src/app/pr_review.rs`), stored on `DiffViewerState.fix_target`
(+ `fix_target_feature_id`) and read by `complete_final_review`.

`dispatch_review_feedback` (`src/app/review.rs:3590`) then routes:

- **`ExistingLive` / `ExistingFeature`** — `paste_review_prompt(session,
  window)` (`src/app/review.rs:3730`) pastes `REVIEW_FEEDBACK_PROMPT`
  (`src/app/review.rs:20`) into the resolved agent's tmux window; sends `Enter`
  when `final_review_submit_prompt` is set; on a submitted paste registers
  `AwaitingReviewFix` so a later idle transition raises a "fixes ready —
  re-review?" notification.
- **`DedicatedReview`** — opens `AppMode::ReviewHarnessPick` for the reviewer to
  choose the harness, then creates/reuses the `"Final Review"` session
  (`FINAL_REVIEW_SESSION_LABEL`, `src/app/review.rs:16`) and pastes there.
- **`NewFeature`** — dispatches into an **isolated companion review feature**
  (its own worktree, a `ReviewSource` back-link, `src/project.rs:396-433`);
  recreates its `"Final Review"` window if gone; **hard-stops with no fallback**
  if the companion id no longer resolves, because falling back into the
  reviewed feature would break the isolation guarantee. A follow-up
  `AppMode::ReviewIntegrate` overlay (`TriageIntegrateState`,
  `src/app/review_destination.rs:629`) then pushes or cherry-picks the fixes
  back to the source branch.

`REVIEW_FEEDBACK_PROMPT` tells the agent to read
`.claude/final-review-feedback.md`, address every item in the most recent
`## Review` section, and append a one-line `**Agent:** …` reply directly under
each item. Those replies are parsed back out on the next round
(`parse_agent_responses` / the `**Agent:**` block handling around
`src/app/review.rs:762` and `:4821`) and shown to the reviewer beside the
changes.

### 1.6 What this means for the investigation

The expensive, repeated read/rewrite loop the plan targets is **§1.2** — the
feature agent maintaining `.claude/review-notes.md` batch by batch during
Review Mode. It is *not* `final-review-feedback.md` (AMF writes that; the agent
reads it once per round) and *not* the diff-viewer walk (no agent at all).

So the concrete optimisation question has two halves:

1. **Can something other than the primary feature agent produce
   `review-notes.md` sections** — from the diff, the agent's turn transcript, or
   a cheap dedicated call — well enough that the developer-notes panel stays
   useful? (Families 1–2, §3.1–3.5.)
2. **Or can the primary agent keep authoring the notes, but stop paying the
   part that actually costs** — the per-batch *read* of the growing file and its
   context carry (§2)? (Family 3, §3.6 — and per §3.6 that read is redundant
   with dedup AMF already runs every turn.)

The remaining sections evaluate both; the recommendation (§7) lands on the
second.

---

## 2. Cost baseline

### 2.1 What is being priced

The recurring cost of §1.2 is the feature agent, batch after batch, **reading**
`.claude/review-notes.md` and **writing** new sections back. Mechanically, in a
Claude Code session that means, per batch:

1. a `Read` tool call whose result injects the whole current notes file into
   context — where it then rides along, re-sent on every later turn until
   compaction;
2. an `Edit`/`Write` tool call whose arguments (the new section prose) are
   billed as output, and whose result re-enters context;
3. some extra reasoning to decide what to write.

Between turns, `archive_review_notes` rewrites the file on disk underneath the
agent (§1.2), so the agent genuinely must re-`Read` each batch rather than
trusting what it already saw.

### 2.2 Assumptions (no live measurement was taken)

No running AMF instance with a billable session was available for this
investigation, so the numbers below are **analytical estimates**, not
measurements. They model a **Claude Code** session, since Review Mode's
instruction only reaches Claude today (§1.2). Parameters:

| Symbol | Meaning | Small-diff value | Large-diff value |
| --- | --- | --- | --- |
| `F` | changed files in the branch | 5 | 60 |
| `B` | logical batches over the whole implementation (`≈ F/3`) | 2 | 18 |
| `s` | tokens per note section (heading + 2 sentences + rule) | ~55 | ~55 |
| `T` | assistant turns in the session after Review Mode is on | 40 | 300 |
| cap | `MAX_LIVE_REVIEW_NOTE_FILES` live-file ceiling | 50 (never hit) | 50 (hit ~batch 15) |
| — | compaction events | 0 | 1 (mid-session) |

Pricing = AMF's default table (`TokenPricingConfig::default`,
`src/token_tracking.rs:858`), i.e. Claude Sonnet-class: input **$3.00**/Mtok,
output **$15.00**/Mtok, cache-read **$0.30**/Mtok, cache-write **$3.75**/Mtok.
The `CLAUDE.local.md` Review-Mode instruction block is ~240 tokens.

### 2.3 Estimate

| Cost component | Small diff (F=5) | Large diff (F=60) |
| --- | --- | --- |
| Instruction block re-sent each turn (cache-read) | ~$0.003 | ~$0.02 |
| Fresh reads of the growing notes file (`B` injections, cache-write) | ~$0.002 | ~$0.11 |
| Section writes (output tokens + tool-call scaffolding) | ~$0.006 | ~$0.10 |
| Notes content carried in context for the rest of the session (cache-read × turns) | ~$0.003 | ~$0.14 |
| Extra reasoning to decide what to note | ~$0.006 | ~$0.07 |
| **Total** | **≈ $0.02** | **≈ $0.45 – $0.75** |
| Cost-equivalent input tokens | ~1.5–2 k | ~150–250 k |

Large-diff total is a range because the notes content can also *trigger*
compaction earlier than it would otherwise happen; if it does, the cost shifts
into a full context re-summarisation that is harder to bound (plausibly another
$0.10–$0.30).

### 2.4 Findings

- **The dominant terms at scale are (a) the `B` fresh reads of the growing
  file and (b) that content riding along in context for the rest of the
  session.** Output/write cost is secondary; the instruction-block cost is
  negligible.
- **The 50-file live cap plus per-turn `archive_review_notes` are load-bearing.**
  They are what keep each read bounded; without them the large-diff number would
  be materially worse and would scale with `F` instead of flattening.
- **The plan's premise — "the notes-file loop dominates final-review cost" — is
  only partly supported.** For small changes the loop is in the noise (~$0.02).
  For a large multi-file branch it is a real but modest fraction: order $0.5
  against an implementation session that might itself run $5–$30. It is a
  *worthwhile* saving, not a dominant one.
- **Biggest uncertainty: `B` and `T`.** If agents batch loosely (many small
  batches), read count and context-carry both rise roughly linearly. A
  pathological "note after every file" agent (ignoring the "batch it"
  instruction) pushes the large-diff figure toward $1.5–$2.
- Recommended update to `AMF_PLAN.md` risk list: mark "the premise that the
  read/rewrite loop dominates cost is an assumption" as **qualified — material
  only on large branches; negligible on small ones.**

These figures are the baseline the options in §7 are compared against.

## 3. Orchestration options for subagents

### 3.0 Framing: three families

AMF today does **not** orchestrate any harness's native subagents. The only
place subagents appear in the code is *accounting*: `src/token_tracking.rs:346`
and `:458` sum `~/.claude/projects/<session>/subagents/*.jsonl` into a session's
usage. Every out-of-band model call AMF makes goes through `HeadlessRunner`
(`src/headless.rs:201`), which spawns a one-shot CLI process, feeds the prompt
on stdin, and holds a `HeadlessLease` (`src/headless.rs:524`) for the
concurrency gate.

The options split into three families:

- **Agent-driven delegation** — AMF changes only the *instructions* it injects
  (the `CLAUDE.local.md` Review-Mode block, §1.2), telling the **primary agent**
  to hand notes writing to its own native subagent. AMF spawns nothing; it
  relies on the harness having a subagent primitive (Claude Code's Task tool;
  OpenCode subagents). — Option A.
- **AMF-driven offload** — AMF runs the notes writer itself, out of the primary
  agent's process: a `HeadlessRunner` one-shot, a dedicated long-lived session,
  or a hook script. Harness-neutral in principle, but each mechanism has a
  different reach across the four harnesses (§5). — Options B, C, D.
- **Reduce the loop in place** — no subagent, no offload; change *what the
  Review-Mode instruction asks the primary agent to do* so the expensive parts
  (the read of the growing file; that file then riding along in context) stop
  happening, while the agent keeps authoring the notes. — §3.6, Options F–K.
  This family exists because §2 found the **read**, not the write, is the
  dominant cost, and the read turns out to be **redundant** with machinery AMF
  already runs (§3.6).

### 3.1 Option A — Claude Code native subagent (Task tool), agent-driven

**Invocation.** The Review-Mode instruction block is rewritten to: *"When you
finish a logical batch, spawn a subagent (Task tool) with the batch's changed
files and a one-line rationale each; instruct it to append the sections to
`.claude/review-notes.md`. Do not read that file yourself."* The primary agent
issues one `Task` call per batch; the subagent runs with its own context
window, does the read + append, and returns a short confirmation.

**Supported harnesses.** Claude Code only (Task tool). OpenCode has an analogous
subagent concept that could be targeted by a harness-variant of the prompt; Codex
and Pi have no subagent primitive (§5).

**Context isolation.** Strong. The subagent's read of `review-notes.md` and its
prose generation happen in a separate transcript; only a ~1-line result returns
to the primary context. The `review-notes.md` content never enters the primary
window. Subagent usage is still billed and still visible to AMF via the
`subagents/*.jsonl` accounting path.

**Failure modes.**
- The primary agent still has to *describe* the batch to the subagent — the
  changed-file list and a rationale each. That rationale is exactly the prose we
  wanted to avoid making it write; the saving is the *read* and the *format
  wrangling*, not the thinking. (Quantified in §7.)
- Non-determinism: the model may ignore the "don't read it yourself" instruction,
  or inline the work instead of spawning, especially under context pressure.
- A subagent that fails (quota, tool-permission, crash) leaves the batch
  undocumented and the primary agent may not notice.
- Subagents cost a fresh system prompt + tool preamble per spawn (~1–3 k tokens
  of cache-write each); at 18 batches that overhead can rival the baseline it
  removes.
- No AMF hook fires on Task-tool completion for *this* purpose;
  `SubagentStop` exists but AMF does not wire it, and it carries no structured
  "did the notes get written" signal.

### 3.2 Option B — AMF-driven `HeadlessRunner` one-shot per batch/finding

**Invocation.** A new gated call site (like the `review.walkthrough` generator
at `src/app/review.rs:1595`): on an end-of-turn IPC notification for a
`feature.review` feature (the same hook point as
`archive_review_notes_after_agent_turn`, `src/app/notifications.rs:367`), AMF
diffs the worktree, finds files changed since the last notes pass, and runs
`HeadlessRunner::run(harness, workdir, prompt, restricted=false)` — or
`run_read_only` — with a "write review notes for these files" prompt. The
one-shot reads the diff (and `review-notes.md`) and emits the sections; AMF
writes them to the file. The primary agent is never asked to touch
`review-notes.md` at all — its Review-Mode block is removed.

**Supported harnesses.** All four. `HeadlessRunner` already has per-harness
command specs for Claude, Codex, OpenCode, Pi (`command_for`,
`src/headless.rs:1060`). `restricted=true` / `run_read_only` give a
no-tools / read-only-repo variant on every harness
(`read_only_command_for`, `src/headless.rs:1187`).

**Context isolation.** Total — it is a different process with a fresh context
every run. Nothing about notes writing ever appears in the primary agent's
window. This is the cleanest isolation of the four.

**Failure modes.**
- **The output must be trusted without the primary agent re-reading it** (this is
  the plan's key open question). If the reviewer, in the diff viewer, sees a
  notes section that is wrong, there is no primary-agent round-trip to correct
  it — the human just discounts it. Acceptable for a *reviewer aid*, less so if
  notes are load-bearing.
- A one-shot that only sees the *diff* (not the agent's reasoning) writes
  shallower "what changed" notes and cannot write "why". Feeding it the agent's
  turn transcript closes that gap but re-introduces a large read — on the
  headless side, where it is at least off the primary's context.
- `HeadlessLease` counts against the agent-concurrency gate
  (`src/resources/limits.rs`); a burst of per-batch runs on a busy machine can
  trip the pre-start resource gate for *other* features.
- Cost/latency per run: a fresh CLI process, model spin-up, and diff read every
  batch. At 18 batches this is 18 cold starts; batching to "once at feature stop
  / once before review" trades freshness for far less overhead.
- Model/harness drift: a Codex `exec` run has no no-tools mode
  (`run_read_only` returns the ordinary ephemeral read-only sandbox for Codex,
  `src/headless.rs:1207`), so behaviour differs subtly per harness.

### 3.3 Option C — dedicated long-lived notes-writer session

**Invocation.** AMF creates a second tmux-backed agent session on the feature
(the way `create_dedicated_review_session` / `FINAL_REVIEW_SESSION_LABEL`
already makes a "Final Review" session, `src/app/review.rs:16`, `:3674`),
labelled e.g. "Review Notes". After each primary-agent turn AMF pastes a short
"document the files changed since your last note" prompt into that session's
window. The session keeps its own context across the whole implementation, so it
accumulates understanding of the change.

**Supported harnesses.** Any tmux-backed agent harness (all four run in tmux
sessions today). No subagent primitive needed.

**Context isolation.** Good *for the primary agent* — its window never sees
`review-notes.md`. But the notes-writer session is itself a full, growing agent
context: it pays the same "file rides along" cost the baseline describes, just
on a second meter. Net tokens across both sessions may not drop; what drops is
the *primary* session's context pressure (fewer compactions, longer useful life).

**Failure modes.**
- **Doubles the live agent count** for every review-mode feature — directly
  against the `resource_gate` philosophy (`src/app/resource_gate.rs`); the
  pre-start gate would fire far more often.
- Two agents in one worktree: the notes writer running while the primary agent
  edits risks reading a half-written tree, or (if it can write code) racing on
  files. It must be locked to read-only + append-to-notes.
- Prompt-delivery fragility: pasting into a tmux window depends on the session
  being alive and at a prompt; `paste_review_prompt` already has to handle "no
  window" fallbacks (`src/app/review.rs:3661`).
- Idle-detection coupling: AMF would need a new "notes writer finished" signal
  to avoid stacking prompts, similar to `AwaitingReviewFix`
  (`src/app/review.rs:3744`).
- Cost: a long-lived session is the *most* expensive of the four to run; it only
  makes sense if primary-context relief is worth more than raw token spend.

### 3.4 Option D — hook-based append (no model, or minimal model)

**Invocation.** AMF already installs `Stop` / `PostToolUse` hooks into
`.claude/settings.local.json` (`src/app/setup.rs:1146`, `:1206`). Add a
`PostToolUse` hook matched to `Edit|Write` (or a `Stop` hook) that runs a script
which: reads the tool payload / the git diff, and appends a **mechanical**
section per changed file — `## <path> — <n> lines changed` plus a bullet list of
changed hunk headers / function names — with no model call at all. Optionally,
the script makes its own tiny `HeadlessRunner`-style call for a one-sentence
"why", but the cheap version is model-free.

**Supported harnesses.** Claude Code has the richest hook surface. OpenCode has
`.opencode/` hooks. Codex and Pi hook support is limited/absent — for those,
the same script can be run by AMF on the end-of-turn IPC notification instead
(so this degrades to "AMF-driven, model-free" rather than being unsupported).

**Context isolation.** Perfect — the primary agent is never involved and never
told to write notes. Zero primary-context cost. Zero model cost in the
model-free variant.

**Failure modes.**
- **Quality floor.** Mechanical notes are "what changed" (file, line counts,
  symbol names) with **no "why"** and no semantic summary. This is close to what
  `docs/backlog/token-efficiency-plan.md` §7 already proposes ("local
  changed-file and diff metadata … existing on-demand walkthrough generation").
  The diff viewer already has the `g` walkthrough generator for the semantic
  layer.
- Hook reliability: a hook that errors or times out can disrupt the primary
  agent's turn (`PreToolUse` diff-review already carries a 600 s timeout,
  `src/app/setup.rs:1190`). A `PostToolUse`/`Stop` script must be fast and
  must never exit non-zero in a way that blocks.
- Per-`Edit` firing is noisy; needs debouncing to per-turn (a `Stop` hook is
  naturally per-turn; `PostToolUse` needs a guard).
- Harness inconsistency: the hook payload shape and available events differ per
  harness, so the "hook" path is really "Claude/OpenCode hook, AMF-side script
  elsewhere".

### 3.5 Summary table (families 1–2)

| | Who spawns | Harnesses | Primary-context cost | Model cost | "Why" quality | Main risk |
| --- | --- | --- | --- | --- | --- | --- |
| A — native subagent | primary agent | Claude (OpenCode partial) | low (batch description only) | subagent spawn overhead ×B | good | non-determinism; spawn overhead |
| B — headless one-shot | AMF | all 4 | ~zero | cold start ×B (or ×1 if batched) | medium (good if fed transcript) | output trusted un-reviewed; concurrency gate |
| C — dedicated session | AMF | all 4 (tmux) | ~zero for primary | highest (second full context) | best | doubles agent count; two agents/worktree |
| D — hook append | AMF (hook/script) | Claude/OpenCode hook; AMF-side elsewhere | zero | zero (model-free) or tiny | low ("what", not "why") | quality floor; hook reliability |

### 3.6 Family 3 — reduce the loop in place (no subagent, no offload)

§2 found the dominant cost is the agent **reading** the growing notes file each
batch, plus that content then **riding along in context** for the rest of the
session — not the writing. And that read is **already redundant**:

- `parse_review_notes` (`src/app/review.rs:5213`) inserts `path -> body` into a
  map as it scans, so a later section for a path silently overwrites an earlier
  one — **newest wins**.
- `split_overflow_review_notes` (`src/app/review.rs:5121`) walks sections
  newest-first, keeps only the newest per path (up to
  `MAX_LIVE_REVIEW_NOTE_FILES`), and routes every superseded copy to overflow.
- `archive_review_notes` (`src/app/review.rs:5153`) applies that split and moves
  the overflow to `review-notes-archive.md` — and it runs **after every agent
  turn** via `archive_review_notes_after_agent_turn`
  (`src/app/notifications.rs:367`), plus once at setup.
- `load_review_notes` (`src/app/review.rs:5196`) merges archive then live, live
  winning, so the diff viewer always shows the newest note per file.

In other words: AMF already de-duplicates and supersedes notes by path on every
turn. The instruction telling the agent to read the file first *"to skip a file
that already has a note"* is asking it to pay for a dedup AMF then redoes anyway.

The in-place options:

| # | Change | Cost removed | Keeps agent "why"? | Effort | Notes |
| --- | --- | --- | --- | --- | --- |
| **F — blind append** | Instruction becomes: *"append a section per file you touched this batch; **do not read `review-notes.md`** — AMF keeps only your latest note per file. Use your own memory of this session to skip a file you've already covered."* | the per-batch **read** and its **context carry** — i.e. both dominant terms of §2 | **yes** | ~1-line instruction change to `ensure_review_claude_md`; **no new code** | machinery already supports it (above). Within one turn the live file can briefly hold duplicates; the post-turn archive pass collapses them. Agent can't *refine* a prior note, only append a fuller replacement — newest wins, which is usually more accurate anyway |
| **G — end-of-turn harvest** | Ask the agent to end each turn with terse `NOTE: <path> — <why>` lines; the existing `Stop` hook scrapes them into the file | the read **and** most of the write (the lines are ~free — the agent already narrates this) | yes | small `Stop` hook script (Claude/OpenCode); AMF-side scrape of the transcript for Codex/Pi | overlaps Option D but agent-authored; no separate "notes prose" pass |
| **H — note once, at stop / before review** | Move the trigger from per-logical-batch to a single pass at feature stop or when `f` opens the review | turns B reads+writes into 1 | yes | instruction + a single prompt/paste at the trigger point | already floated in `token-efficiency-plan.md` §7 ("request it once before review or feature stop"); loses per-batch "why" granularity |
| **I — AMF says which files need a note** | AMF (which has the diff) tells the agent the changed-file list and filters out renames / pure formatting / trivial one-liners; agent notes only the rest | fewer sections → less output and a smaller file (smaller reads if any remain) | yes | AMF-side diff classification + inject the list | composes with F/H |
| **J — AMF writes "what", agent adds "why"** | AMF appends the mechanical skeleton (file + hunk headers) after each turn; the agent appends only a `why:` line where something is non-obvious | the agent's cost of deciding + phrasing "what changed" | yes, where it matters | AMF-side mechanical writer (same as Option D's core) + a shorter instruction | Option D's Layer-1 writer plus a cheap agent-driven "why", with no headless call |
| **K — terser format** | `- <path>: <why>` instead of `## <path> — <title>` + 1–2 sentences + `---` | ~half the write cost; less parse ambiguity | yes | instruction + a `parse_review_notes` heading variant | small standalone win; composes with everything |

**Failure modes for family 3 as a whole.**
- These are *instruction* changes: compliance is probabilistic. But a
  non-compliant agent under family 3 fails toward *the current behaviour*
  (it reads the file anyway) — a safe degradation, unlike Option A where
  non-compliance means notes silently don't get written.
- Option F trades away the agent's ability to see its own older notes after a
  compaction; it writes a fresh note from the current diff instead. Acceptable.
- Option G/J depend on hook or IPC reliability the same way Option D does.
- None of family 3 helps a harness where Review Mode's `CLAUDE.local.md` block
  isn't read — but that is every supported harness's designated local-instruction
  file, so this is not a real gap.

### 3.7 Summary table (family 3)

| | Primary-context cost | Model cost | "Why" quality | Effort | Harnesses |
| --- | --- | --- | --- | --- | --- |
| F — blind append | ~zero (no read, no carry) | write-only (small) | agent-authored | trivial (instruction) | all 4 |
| G — end-of-turn harvest | ~zero | ~zero marginal | agent-authored, terse | small (hook/scrape) | all 4 |
| H — once at stop/review | one read+write total | one write pass | agent-authored, coarser | small | all 4 |
| I — AMF picks files | lower (smaller file) | less output | agent-authored | small (AMF-side) | all 4 |
| J — AMF "what" + agent "why" | ~zero | tiny (agent `why:` only) | agent where it matters | medium (AMF writer) | all 4 |
| K — terser format | lower | ~half write | agent-authored | trivial | all 4 |

## 4. Review-files spec options

Headline: **the file spec should not change.** The current shape is already the
right one for every axis below; the investigation's leverage is entirely on
*who writes the file*, not on the file. Each axis, with the options and the
recommended default:

### 4.1 Format — **Markdown (keep)**

| Option | For | Against |
| --- | --- | --- |
| **Markdown, `## <path> — <title>` sections (current)** | already parsed by `parse_review_notes` (`src/app/review.rs:5213`) and `review_note_sections` (`:5060`); listed as a viewable doc in `src/markdown.rs:20`; human opens it directly; a one-shot / subagent emits it with no schema | loose parsing; a malformed heading silently drops a section |
| JSON (`[{path, title, why, ...}]`) | machine-robust; easy for AMF to validate a subagent's output before writing | new parser + new writer; loses the "just open the file" property and the markdown-viewer integration; not reviewer-readable raw |

**Recommendation:** keep Markdown as the on-disk surface. If a subagent's output
needs validating, have it emit Markdown and let AMF normalise on write — it
already round-trips sections through `review_note_sections` /
`write_review_notes_atomic`.

### 4.2 Granularity — **single file, one section per changed file (keep)**

| Option | Verdict |
| --- | --- |
| **Single file, section per changed file (current)** | matches the diff viewer's per-file panel exactly (`state.review_notes: HashMap<path, note>`) and `save_review_snapshot`'s per-path keying — **keep** |
| One per *finding* | category error — notes are the *author's* rationale, findings are the *reviewer's*; there is no "finding" at notes-writing time — reject |
| One file per changed file (`.claude/review-notes/<path>.md`) | would ease concurrent writes and per-file staleness, but breaks `load_review_notes` / archive / markdown-viewer and multiplies gitignore + FS churn; the atomic-replace already handles the single-writer case — reject |

### 4.3 Location — **`.claude/review-notes.md` in the feature worktree (keep)**

| Option | Verdict |
| --- | --- |
| **`<workdir>/.claude/review-notes.md` (current)** | same dir as every other review sidecar (`final-review-progress.json`, `final-review-snapshot.json`, `final-review-feedback.md`); already resolved by `load_review_notes(workdir)`; already gitignored via `.claude/.gitignore` — **keep** |
| Temp dir | loses worktree association, does not survive an AMF restart; nothing else in the review flow lives outside `.claude/` — reject |
| `.worktrees/` | wrong scope — that is the *container* of sibling worktrees, not a per-feature location — reject |

### 4.4 Committed vs gitignored — **gitignored (keep), with a caveat to resolve**

Keep it gitignored (as `ensure_notification_hooks` / `ensure_review_claude_md`
already arrange, `src/app/setup.rs:1239-1242`, `:1394-1395`). Notes are a
transient reviewer aid, not source.

**Caveat for the follow-up.** A gitignored `review-notes.md` does **not** travel
to a companion review feature: `create_review_companion_feature`
(`src/app/review_destination.rs:421`, worktree created at `:476`) branches a
fresh worktree from the base
SHA, and both `review-notes.md` and `final-review-feedback.md` are gitignored
*and* are written to the **source** feature's workdir
(`persist_final_review_round(&workdir, …)` uses the reviewed feature's workdir,
`src/app/review.rs:3240` / `:3514`). So the companion agent that
`dispatch_review_feedback` pastes `REVIEW_FEEDBACK_PROMPT` into is pointed at a
`.claude/final-review-feedback.md` that is not in its worktree. This is a
**pre-existing** characteristic of the feedback file, independent of this
investigation — but any subagent-notes design that expects the companion flow to
surface notes must copy the file into the companion worktree explicitly (or
choose a committed location for that flow). **Flag for maintainer:** confirm
whether the companion currently receives `final-review-feedback.md` at all, and
mirror that decision for notes.

### 4.5 Lifecycle — **retain + archive (keep)**

| Option | Verdict |
| --- | --- |
| **Retain live, archive overflow (current)** — `MAX_LIVE_REVIEW_NOTE_FILES = 50`, per-turn `archive_review_notes` (`src/app/review.rs:69`, `:5153`) | notes stay visible across re-review rounds; live read stays bounded; archive keeps history for `load_review_notes` — **keep** |
| Shown-then-deleted | loses the re-review value — the diff viewer shows notes on every round, not just the first — reject |
| Overwritten per run | loses history the reviewer relies on when the agent iterates — reject |

A subagent/offload design changes *who* appends and *when the archive pass runs
relative to that write* (it needs a lock or an ordering guarantee against
`archive_review_notes`), but not the lifecycle policy itself.

### 4.6 Net

Recommended files spec = **exactly today's**: Markdown, single file, section per
changed file, `<workdir>/.claude/review-notes.md`, gitignored, retain + archive.
Carry one open item — companion-worktree propagation (§4.4) — into the
follow-up's open questions.

## 5. Harness support matrix

### 5.1 Matrix

`AgentKind` has exactly four variants — `Claude`, `Opencode`, `Codex`, `Pi`
(`src/project.rs:107-113`). The codebase carries **no** notion of subagent
support for any of them today; the "native subagent" column below is from each
tool's documented capabilities as of this writing and should be re-verified when
the follow-up starts.

**Starting point: Review Mode is Claude-only today.** `ensure_review_claude_md`
writes only `CLAUDE.local.md` (§1.2), so on a Codex / OpenCode / Pi feature the
notes instruction is written but not read, and no `review-notes.md` loop
happens. Two consequences: the cost this investigation targets currently exists
only on Claude features, and "extend review notes to all four harnesses" is a
*separate* decision (make `ensure_review_claude_md` harness-aware, mirroring
`ensure_plan_mode_instructions`) that any option here depends on but none of
them delivers.

| Harness | Native subagent primitive | `HeadlessRunner` one-shot | Restricted / read-only one-shot | Notes |
| --- | --- | --- | --- | --- |
| **Claude Code** | **Yes** — Task tool + `.claude/agents/*.md` | Yes (`command_for` → `claude -p`, `src/headless.rs:1062`) | Yes — `--safe-mode --tools ""` (restricted) / `--tools Read,Glob,Grep` (read-only), `src/headless.rs:1070`, `:1189` | Full hook surface (`Stop`, `PostToolUse`, `SubagentStop`, …) already managed by AMF (`src/app/setup.rs:1146`+) |
| **OpenCode** | **Partial** — subagents via `opencode.json` / `.opencode/agent/*.md`, `task` tool / `@mention` | Yes (`opencode run`, `src/headless.rs:1095`) | Yes — `--pure` + `OPENCODE_PERMISSION` deny-all, `src/headless.rs:1098`, `:1208` | Has an `.opencode/` hook equivalent; AMF must not touch global `~/.config/opencode/` (CLAUDE.md rule) |
| **Codex** (`codex exec`) | **No** — single agent, no delegation primitive | Yes (`codex exec --sandbox read-only --ephemeral`, `src/headless.rs:1079`) | Yes, but **only** the ephemeral read-only sandbox — there is no separate no-tools mode (`run_read_only` returns the ordinary command for Codex, `src/headless.rs:1207`) | Limited hook support |
| **Pi** | **No documented primitive** — treat as none until verified | Yes (`pi -p --no-session`, `src/headless.rs:1112`) | Yes — `--no-tools --no-extensions --no-skills --no-prompt-templates --no-context-files --no-approve`, `src/headless.rs:1119`, `:1214` | `--no-tools` covers built-in + extension + custom tools on current releases |

### 5.2 What the optimisation can and cannot do per harness

- **Claude Code** — every option (§3) is available. Option A (native subagent)
  is viable; Options B/C/D are all viable and already have plumbing
  (`HeadlessRunner`, `create_dedicated_review_session`, managed hooks).
- **OpenCode** — Options B/C/D fully; Option A possible but needs an
  OpenCode-specific delegation prompt and an `opencode.json` subagent
  definition AMF would have to write into `.opencode/` (worktree-local, never
  global).
- **Codex** — Options B (headless one-shot) and D-as-AMF-side-script only.
  No native subagent; hook support too thin to rely on. Option C (dedicated
  tmux session) technically works but Codex has no read-only *interactive*
  mode, so a second Codex session in the worktree could edit code — it would
  have to be trusted or fenced by prompt alone.
- **Pi** — same as Codex: Options B and D-as-AMF-side-script. `--no-tools`
  headless is clean, so a Pi one-shot notes writer that receives everything in
  its prompt is well-supported; anything needing repo tools is `run_read_only`
  with Pi's read-only tool set.

### 5.3 How harnesses without native subagents are handled

**Not "unsupported".** Two cases:

- **If review notes stay Claude-only** (the status quo — §5.1), the question is
  moot: there is no non-Claude loop to optimise, and the recommended Option F
  (§7) is a Claude-instruction edit that changes nothing for the others.
- **If review notes are extended to all four harnesses** (by making
  `ensure_review_claude_md` harness-aware), Family 3 (Option F etc.) extends with
  it — every supported harness reads *some* local-instruction file. The
  AMF-driven offload (Options B / D / J) also covers all four and is the only
  path that works with **no** instruction file at all. Native subagents
  (Option A) remain a Claude/OpenCode-only enhancement, never the load-bearing
  mechanism — so the plan's "any harness with native subagents" requirement is
  met structurally either way.

Recommended posture (aligned with §7 — Option F first, AMF-driven build only if
it proves necessary):

| Harness | Path | Degradation |
| --- | --- | --- |
| Claude Code | Option F (instruction edit); AMF-driven build (D/J + optional headless "why") only if F is insufficient; native subagent (A) only if it measures better | none |
| OpenCode | needs `ensure_review_claude_md` harness-awareness first; then Option F; AMF-driven build as the Claude fallback | notes absent until the instruction is delivered |
| Codex | as OpenCode; no native-subagent variant | as OpenCode |
| Pi | as OpenCode; `--no-tools` headless makes an AMF-driven writer clean here | as OpenCode |

So there is **no degraded feature tier** for the core saving *once the
instruction reaches a harness* — the primary agent stops reading
`review-notes.md`. The only cross-harness gap is instruction delivery
(Claude-only today), which is a prerequisite, not a property of any option
here.

## 6. Prompt-registry integration

> **Applies to Step 2 only.** The recommended Step 1 (Option F, §7.4) is an edit
> to the `CLAUDE.local.md` instruction block and adds **no** registry prompt.
> This section maps the integration for the *deferred* AMF-driven writer, so the
> design is ready if Step 1 proves insufficient.

### 6.1 What final review uses today

| Prompt | `PromptId` | Key | In final-review flow? | Notes |
| --- | --- | --- | --- | --- |
| File walkthrough | `ReviewWalkthrough` | `review.walkthrough` | Yes — `g` generates a note for a file with none (`src/app/review.rs:1629`) | resolved via `resolve_headless_prompt` but with `AgentKind::Claude` **hardcoded**, dispatched via `ClaudeLauncher::spawn_headless`, not `HeadlessRunner` |
| AI co-reviewer | `ReviewCoReview` | `review.co_review` | Yes — first-pass draft comments | Claude-only in practice (same pattern) |
| Changeset overview | `ReviewChangesetOverview` | `review.changeset_overview` | Yes — `O` triage overview | Claude-only in practice |
| Diff explain | `ReviewDiffExplain` | `review.diff_explain` | No — config-wizard / `PreToolUse` diff-review hook | not final review proper |

All four specs have `harness_variants: NO_HARNESS_VARIANTS`
(`src/prompts/mod.rs:255`+). Every final-review spec's built-in template is the
shared one.

**Crucially, none of these drive the notes round-trip.** The two prompts that
*do* are **not in the registry**:

- The Review-Mode instruction block written into `CLAUDE.local.md` by
  `ensure_review_claude_md` (`src/app/setup.rs:1355-1378`) — a `concat!` string
  constant, no `PromptId`.
- `REVIEW_FEEDBACK_PROMPT` (`src/app/review.rs:20`) — a `const &str`, pasted
  into the fixing agent's tmux window, no `PromptId`.

So a notes-writer prompt is genuinely **new** surface; it does not replace an
existing `PromptId`.

### 6.2 Adding `review.notes_writer` as a `PromptSpec`

1. **`src/prompts/mod.rs`** — add `PromptId::ReviewNotesWriter`; append to
   `PromptId::ALL` (15 → 16 — the `SPECS.len()` / `every_prompt_id_has_a_spec`
   tests enforce the pair stays in sync); add the `as_str` arm
   (`"review.notes_writer"`) and it is picked up by `from_key` automatically.
2. **`SPECS`** — a `PromptSpec` with `title`, `summary` ("Writes
   `.claude/review-notes.md` sections for a review-mode feature, out of the
   primary agent's context"), `placeholders` (e.g. `{{changed_files}}`,
   `{{diff}}`, `{{existing_notes}}`, `{{agent_rationale}}`), `default_template`
   pointing at a new `defaults.rs` constant, and — this is where the plan's
   `harness_variants` requirement lands:

   ```rust
   harness_variants: &[
       (AgentKind::Codex, REVIEW_NOTES_WRITER_INLINE),
       (AgentKind::Pi,    REVIEW_NOTES_WRITER_INLINE),
   ],
   ```

   The shared `default_template` can tell a *subagent* (Claude / OpenCode) to
   read `review-notes.md` and the diff itself; the Codex/Pi variant
   (`REVIEW_NOTES_WRITER_INLINE`) instead expects `{{diff}}` and
   `{{existing_notes}}` fully inlined, because those one-shots run
   `--no-tools` / read-only and have no reliable way to read the file
   mid-run. This is exactly the use `harness_variants` was reserved for —
   "a harness-specific default can be added later without touching call sites"
   (`src/prompts/mod.rs:146-148`).
3. **`defaults.rs`** — the template text, beside `REVIEW_WALKTHROUGH` etc. A
   drift-guard test is only needed if the text must stay in sync with prose
   living elsewhere (the plan-interview pattern); a fresh standalone prompt
   just needs the round-trip test (`every_prompt_id_has_a_spec`,
   `prompt_keys_are_stable_and_unique`).
4. **Call site** — resolve with the **feature's actual harness**, not a
   hardcoded `Claude`:

   ```rust
   let prompt = self.resolve_headless_prompt(
       PromptId::ReviewNotesWriter, &harness, &repo, &workdir, &ctx);
   // automated (per-turn) → toast, not the modal:
   self.announce_headless_run(/* … */);
   let notes = HeadlessRunner::run(&harness, &workdir, &prompt, /*restricted=*/false)?;
   ```

   Dispatch through `HeadlessRunner` (not `ClaudeLauncher`) so all four
   harnesses are reachable (§5). Because it is an **automated** run (fires on
   the end-of-turn IPC notification, like `archive_review_notes_after_agent_turn`),
   it uses `announce_headless_run` / a toast — never `precall_gate`'s modal,
   which "a queued batch can't deadlock" (CLAUDE.md precall section). It still
   needs a `PrecallAction` variant for the announce path.

### 6.3 Override-layering implications

`review.notes_writer` gets the standard three-scope treatment for free once it
is in `PromptId::ALL`:

- **Resolution:** `resolve_template_layered` — feature (DB, keyed by workdir
  path) → project (`amf.json` `ExtensionConfig::prompt_overrides`) → global
  (DB) → built-in; nearest wins; within the winning layer a per-harness
  template beats the shared one (`src/prompts/resolve.rs:141`). Read fresh each
  call (`resolve_headless_template`, `src/app/mod.rs:3261`).
- **Manager overlay:** appears in the dashboard `E` / leader `E` list
  automatically (it iterates `all_specs()`); editable per scope; `d,d` clears
  the effective override.
- **Team override:** a project can set house style for notes in committed
  `amf.json` (`src/prompts/project.rs`); an individual overrides per-feature or
  globally in `amf.db` without affecting the team.
- **Silent default drift (documented caveat):** "Once any layer supplies an
  override the built-in default is never read." A user who customises
  `review.notes_writer` will not pick up a later improvement to AMF's built-in
  template — same trade-off as every other registry prompt, but it bites harder
  here because the prompt runs **automatically every turn**: a stale or broken
  override degrades silently rather than being noticed at an interactive
  moment.
- **Unvalidated interpolation:** an override may drop `{{diff}}` /
  `{{changed_files}}` freely; missing tokens render literally
  (`render_template`). For an automated prompt this means a malformed override
  produces bad notes on every turn with no error. The follow-up should add a
  lightweight "resolved notes-writer prompt still references its key tokens"
  warning in the manager overlay, or accept the risk explicitly.
- **Out of scope but noted:** migrating the Review-Mode `CLAUDE.local.md` block
  and `REVIEW_FEEDBACK_PROMPT` into the registry (so they too become
  overridable) is a reasonable follow-on but is not required for the notes
  optimisation and would widen the change surface considerably.

## 7. Comparison and recommendation

### 7.1 The field of options

Three families (§3): **A** agent-driven native subagent; **B/C/D** AMF-driven
offload (headless one-shot / dedicated session / hook append); **F–K**
reduce the loop in place (§3.6). `docs/backlog/token-efficiency-plan.md` §7
already points at the last family — drop the per-batch instruction and lean on
local metadata + the on-demand `g` walkthrough.

### 7.2 Scoring against the baseline (§2)

Ratings: ✅ strong / ➖ mixed / ❌ weak.

| | Primary-context relief | Net token change vs baseline | Impl. complexity | Harness coverage | Robustness / trust | UX impact |
| --- | --- | --- | --- | --- | --- | --- |
| **A** native subagent | ✅ high | ➖ maybe *worse* on large diffs (spawn overhead ×B + primary still describes each batch) | ➖ medium (prompt-only, non-deterministic; no test infra) | ❌ Claude only (+OpenCode maybe) | ❌ model may not delegate; silent subagent failure | ➖ invisible when it works, confusing narration when not |
| **B** headless one-shot (batched ×1) | ✅ ~total | ✅ well below baseline (one cold start, nothing persists) | ➖ medium (one gated call site; lock vs `archive_review_notes`) | ✅ all 4 | ➖ output trusted un-reviewed; "why" needs the transcript fed in | ➖ toast + latency before notes appear |
| **C** dedicated session | ✅ ~total for primary | ❌ up (second full context) | ❌ high (lifecycle, paste fragility, idle signal, read-only fence, resource gate) | ✅ all 4 (tmux) | ➖ two agents / worktree race risk | ❌ unrequested session; against `resource_gate` |
| **D** hook append (model-free) | ✅ total | ✅ ~zero added | ➖ low–medium (script + hook / AMF-side call) | ✅ all 4 | ✅ can't hallucinate; fails to "no note" | ➖ "what", no "why" (`g` covers semantics) |
| **F** blind append (no read) | ✅ ~total (no read, no carry) | ✅ well below baseline (write-only) | ✅ **trivial — a one-line instruction change, no new code** | ✅ all 4 | ➖ instruction compliance; **fails toward today's behaviour** | ✅ none — notes look exactly as they do now |
| **G** end-of-turn harvest | ✅ ~total | ✅ ~zero marginal | ➖ low (hook/scrape) | ✅ all 4 | ➖ hook/IPC reliability (like D) | ✅ ~none |
| **H** note once at stop/review | ✅ high (1 read+write, not B) | ✅ well below baseline | ✅ low | ✅ all 4 | ➖ compliance; coarser "why" | ➖ notes appear later / in one lump |
| **J** AMF "what" + agent "why" | ✅ ~total | ✅ ~zero + tiny agent `why:` | ➖ medium (AMF mechanical writer) | ✅ all 4 | ✅ mostly mechanical + small agent add | ➖ ~none |
| **K** terser format | ➖ lower (smaller file) | ✅ ~half the write | ✅ trivial | ✅ all 4 | ✅ nothing new | ✅ denser panel |

### 7.3 Reading the scores against the premise

§2 found the loop is a **modest** cost — negligible on small changes, order
$0.5 on a large branch, and the **read** (plus its context carry), not the
write, is the dominant term. That, plus §3.6's finding that **the read is
redundant with dedup AMF already runs every turn**, reframes the whole problem:

- It rules **C** out: a second full agent context to save ~$0.5 is a bad trade
  and fights `resource_gate`.
- It weakens **A**: a Claude-only mechanism that may not reduce net spend on the
  case that matters is not worth the non-determinism.
- It makes a **heavy AMF-driven build (D + a headless "why" pass) look
  disproportionate** to the size of the win, *if a one-line instruction change
  gets most of the way there.*
- **Option F does get most of the way there.** It removes both dominant terms,
  keeps agent-authored "why" for free, works on all four harnesses, needs no
  new code, and its worst-case failure is "the agent reads the file anyway" —
  i.e. today.

### 7.4 Recommendation — Option F now, AMF-driven build only if it proves necessary

**Step 1 (do this): blind append (Option F), plus Options I and K.**
- Rewrite the Review-Mode block in `ensure_review_claude_md`
  (`src/app/setup.rs:1355-1378`): *append a section per file touched this batch;
  **do not read `review-notes.md`**; rely on your own session memory to skip a
  file already covered.* Optionally switch the section format to the terser
  `- <path>: <why>` form (Option K) with a matching `review_note_path_from_heading`
  variant.
- Have AMF inject the changed-file list and suppress notes for renames / pure
  formatting / trivial one-liners (Option I) — AMF already has the diff.
- Everything downstream is unchanged: `archive_review_notes` still runs every
  turn and still collapses to newest-per-path (§3.6), the diff-viewer panel
  still reads via `load_review_notes`.
- **Cost:** the per-batch read and its context carry — both dominant terms of
  §2 — go to zero. Writes remain, small. No new code paths, no new failure
  modes.
- **Harness reach is unchanged by this** — Review Mode is Claude-only today
  (§1.2 / §5.1), and Option F keeps it that way. Extending review notes to the
  other three harnesses (make `ensure_review_claude_md` harness-aware) is a
  separate, small prerequisite that benefits Option F and Step 2 equally; it is
  not part of this recommendation.

**Step 2 (only if Step 1 is insufficient): the AMF-driven fallback.**
Dogfood a large branch with Option F in place. If agent-authored notes are
still too costly (loose batching), too unreliable (agent keeps reading the file
or skipping notes), or the "why" quality regresses, escalate to the previous
recommendation:
- **Layer 1:** AMF writes model-free mechanical notes on the end-of-turn IPC
  hook (`archive_review_notes_after_agent_turn`, `src/app/notifications.rs:367`);
  the Review-Mode instruction is removed entirely (Option D core / J).
- **Layer 2 (opt-in, default off):** one `HeadlessRunner` `review.notes_writer`
  enrichment pass per review (feature's real harness, §6.2), non-blocking,
  `announce_headless_run` toast, failure leaves Layer 1 intact.

**Step 3 (unchanged, both paths): the `g` on-demand walkthrough** stays as the
reviewer's per-file semantic escape hatch (`src/app/review.rs:1629`).

**Why F first**
- The win is modest (§2), so match the effort to it: a one-line instruction
  change before a multi-part AMF build.
- It keeps agent-authored "why" — the panel's main value over the raw diff —
  at no added cost, instead of trading it away and buying it back with a
  headless pass.
- It fails safe *toward the status quo*, not toward "notes silently missing".
- It doesn't foreclose Step 2: if F underperforms, the AMF-driven build is
  still there, now with real dogfood data on whether the "why" pass is needed.

**Rejected outright:** C (cost + complexity + `resource_gate` conflict); A as
the primary mechanism (harness coverage + non-determinism + spawn overhead);
per-batch B (least efficient B variant). **Deferred:** the full AMF-driven
build (Step 2) — designed, not built, pending Step 1's results.

## 8. Follow-up go/no-go

### 8.1 Verdict: **GO — as a small Step 1, with Step 2 deferred**

The saving is **real but modest** (§2): negligible on small changes, order $0.5
on a large branch, dominated by the file *read*, which §3.6 shows is redundant
with dedup AMF already runs every turn. So the follow-up should **start with
the one-line instruction change (Option F)** and only build the AMF-driven
notes writer if dogfooding shows F is not enough. Nothing here justifies a new
session type or schema change.

### 8.2 Recommended scope for the follow-up feature

**Step 1 — ship this first (small, ~instruction-only)**
1. Rewrite the Review-Mode block in `ensure_review_claude_md`
   (`src/app/setup.rs:1355-1378`): *append a section per file touched this
   batch; **do not read `review-notes.md`**; use session memory to skip a file
   already covered* (Option F). Update `strip_between_markers` handling and the
   block-content tests (`src/app/tests.rs:1124`, `:7603`, `:11492`).
2. Inject the changed-file list and suppress notes for renames / pure
   formatting / trivial one-liners (Option I) — AMF has the diff via the same
   `diff::` plumbing `save_review_snapshot` uses.
3. Optionally switch the section format to the terser `- <path>: <why>` form
   (Option K) with a matching `review_note_path_from_heading` branch and a
   `parse_review_notes` test.
4. Docs: tick `token-efficiency-plan.md` §7, fix the stale "note before every
   Edit/Write" wording (§1.2 drift note), update the
   `final-review-enhancements-plan.md` cross-reference.
5. Verify: `archive_review_notes` still collapses blind-appended duplicates
   (extend `archive_review_notes_moves_overflow_and_is_idempotent` /
   `ipc_turn_end_archives_superseded_review_notes_for_review_features`,
   `src/app/tests.rs:11492`).

**Step 2 — build only if Step 1 dogfooding is insufficient (designed, not
scheduled)**
6. Pure `fn mechanical_review_notes(&[diff::DiffFile]) -> Vec<(path, section)>`
   — file, line delta, changed hunk headers / touched symbols — unit-tested.
7. Call it from the end-of-turn IPC path next to
   `archive_review_notes_after_agent_turn` (`src/app/notifications.rs:367`),
   gated on `feature.review`; write via `write_review_notes_atomic`
   (`src/app/review.rs:5095`), then `archive_review_notes`. Remove the
   Review-Mode instruction entirely at this point.
8. `PromptId::ReviewNotesWriter` / `review.notes_writer` registry entry with a
   shared template + Codex/Pi `harness_variants` (§6.2), plus round-trip tests.
9. Config flag (default **off**) running **one** `HeadlessRunner` enrichment
   pass per review — feature's real harness, `announce_headless_run` toast,
   polled like `poll_review_walkthrough` (`src/app/review.rs:1664`), failure
   leaves the mechanical notes intact.

**Explicitly out of scope (both steps)**
- Option C (dedicated notes-writer session) — do not build.
- Option A (native-subagent delegation) as the mechanism — possible later as a
  Claude-only toggle, not now.
- Migrating the `CLAUDE.local.md` block and `REVIEW_FEEDBACK_PROMPT` into the
  prompt registry.
- Any change to the file spec (§4): format, granularity, location, lifecycle
  stay as they are (Option K is a format *tweak* within the same parser, not a
  spec change).
- Per-changed-file note files, JSON notes, committed notes.

### 8.3 Prerequisites

**Before Step 1**
- **P0 — Confirm blind-append is safe against the archive pass.** Verify (test,
  §8.2.5) that a turn appending duplicate/superseded sections for the same path
  is fully collapsed by `archive_review_notes` on the next turn boundary, and
  that the live file cannot grow unbounded *within* a single very long turn
  (bound it if needed).
- **P0b — Decide the scope: Claude-only or all harnesses.** Review Mode's
  instruction only reaches Claude today (`ensure_review_claude_md` writes only
  `CLAUDE.local.md`, §1.2 / §5.1). If review notes should exist on Codex /
  OpenCode / Pi at all, first make `ensure_review_claude_md` harness-aware
  (mirror `ensure_plan_mode_instructions`, `src/app/setup.rs:1249`+, which
  already writes `AGENTS.md`). Orthogonal to Option F, but it decides whether
  Step 1 is a Claude-only change or a four-harness one.

**Before Step 2 (if reached)**
- **P1 — Re-verify the harness matrix (§5)** against installed CLIs; the
  codebase carries no capability flags.
- **P2 — Companion-worktree propagation (§4.4).** Determine whether a companion
  review feature receives `.claude/final-review-feedback.md` today at all
  (`persist_final_review_round` writes to the *source* workdir; the companion
  is a fresh worktree from base). Make `review-notes.md` behave the same way —
  copy into the companion, or document that notes are source-feature-only.
  *(This one applies to Step 1 too if notes should ever surface in a companion.)*
- **P3 — Verification strategy.** CLAUDE.md's "no tests yet" is stale;
  `src/app/tests.rs` is large and active. Pure functions unit-test there; the
  IPC → write → archive path needs a `tempfile`-backed test like
  `ipc_turn_end_archives_superseded_review_notes_for_review_features`
  (`src/app/tests.rs:11492`). Agree this is sufficient given no integration rig.
- **P4 — Config surface.** Name/scope/default of the enrichment flag
  (`final_review_notes_enrichment`?), living in `ExtensionConfig` /
  per-project `amf.json` beside `final_review_check_command` /
  `final_review_post_to_pr`.

### 8.4 Open questions to carry into the follow-up

1. **Does Option F alone hold up on a large branch?** Dogfood it. Failure
   signals that trigger Step 2: agents batch so loosely the write cost creeps
   back; agents keep reading the file despite the instruction; "why" quality
   drops because the agent no longer sees its earlier notes. If Step 2 *is*
   reached, also decide whether the model-free mechanical notes are enough or
   the headless "why" pass is effectively mandatory.
2. **Trust without primary re-read** (plan risk): confirmed low-stakes here.
   The only two consumers of `load_review_notes` are the diff-viewer
   developer-notes panel (`src/app/diff.rs:158` → `state.review_notes`) and
   `find_review_note` in the `PreToolUse` diff-review prompt dialog
   (`src/handlers/diff_review.rs:322`) — both **human-facing surfaces**. Notes
   never feed an agent prompt and are not stored in `save_review_snapshot`
   (which keeps `new_content`, not notes). A wrong note misleads a human
   momentarily; nothing downstream acts on it. Re-confirm this list before
   building.
3. **Write-ordering** between the Layer 1 mechanical write, a backgrounded
   Layer 2 enrichment write, and `archive_review_notes` — all three target the
   same file. Layer 1 and archive are on the UI thread; Layer 2 lands async.
   Needs a defined "enrichment re-reads, rewrites atomically, re-archives"
   sequence and a guard against an enrichment landing after the review already
   opened.
4. **`agent_rationale` for Layer 2** — feeding the agent's last turn transcript
   gives real "why" but is itself a large read on the headless side. Decide:
   full recent transcript, just the turn's assistant messages, or diff-only
   (shallower notes, cheaper). Cap it by tokens.
5. **Interaction with `resource_gate` / `HeadlessLease`** — a Layer 2 pass
   counts against the agent-concurrency gate; on a busy machine it can defer.
   Acceptable (it is non-blocking and best-effort) but should be stated in the
   flag's docs.
6. **Reviewer-visible provenance** — should the panel distinguish
   AMF-mechanical notes from enriched notes from agent-authored ones (older
   features), so a reviewer knows how much to trust a section? Small UI
   decision, worth making deliberately.
7. **Override drift for an every-turn prompt** (§6.3) — ship the "resolved
   `review.notes_writer` still references its key tokens" warning in the
   manager overlay, or accept silent degradation?
