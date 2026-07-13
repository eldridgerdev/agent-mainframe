# Plan Mode: guided feature discovery interview

- **Status:** In progress
- **Owner:** unassigned
- **Relates to:** current plan mode (`ensure_plan_mode_claude_md` in
  `src/app/setup.rs`, `Feature.plan_mode` in `src/project.rs`), feature
  creation wizard (`src/handlers/feature_creation.rs`,
  `CreateFeatureState` / `CreateFeatureStep` in `src/app/state.rs`),
  extension config (`src/extension.rs`), agent harnesses
  (`AgentKind` in `src/project.rs`; `src/claude.rs`, `src/codex.rs`,
  `src/pi.rs`, headless use in `src/summary.rs`), markdown viewer
  (`src/markdown.rs`,
  `AppMode::MarkdownViewer`), hook prompts (`HookPrompt` in
  `src/extension.rs`, `src/ui/dialogs/hooks.rs`)

## Why / problem

Plan mode today is a boolean. Toggling it on in the feature wizard (or
via a preset) injects a "Plan Mode" block into the workdir's
`CLAUDE.local.md` pointing at a gitignored repo-root `PLAN.md`
skeleton, and the agent is asked to keep that file updated. Nothing
helps the *user* think the feature through before the agent starts:
no intake of goals, no probing of architecture or UI decisions, no
structured plan the user has actually agreed to. The agent starts from
a one-line feature name and improvises.

The goal is a fully fledged discovery flow: AMF asks the user about
the feature — through curated built-in questions, per-project
question templates, and AI-generated adaptive follow-ups — then
synthesizes the answers into a structured plan the user reviews and
edits **before** the implementing agent launches seeded with it.

## Decided design constraints

Settled with the project owner (2026-07-13):

1. **Interview runs natively in AMF's TUI** — questions render as
   native dialogs/overlays before any tmux session launches; answers
   are collected and owned by AMF, not by the agent.
2. **All three question sources**, layered: built-in bank →
   user-defined per-project templates → AI-generated adaptive rounds.
3. **Two triggers**: automatically during feature creation when plan
   mode is on, and on-demand for an existing feature (re-runnable).
4. **Output is an AI-synthesized plan doc behind a review gate**: the
   user reviews/edits the generated plan in AMF before the agent
   session launches. The on-disk home is the workdir's
   `.claude/plan.md` (revised from "PLAN.md stays" after the
   2026-07-13 audit below).

## Existing plan mode: audit verdict (2026-07-13)

Full trace of the ~60 `plan_mode` references answered "does the
existing feature make sense, or remove it?" — the answer is split:

**Keep: the trigger plumbing.** Only two things actually *consume*
the flag: `ensure_feature_running` calls `ensure_plan_mode_claude_md`
on every feature start, and the dashboard list draws a `[plan]`
badge. Everything else — wizard toggle, `FeaturePreset.plan_mode`,
the `features.plan_mode` DB column, the automation IPC
`CreateFeatureRequest.plan_mode` field, the worktree-hook threading —
is plumbing that carries the boolean to those two consumers. That is
exactly the trigger surface the interview needs, and ripping it out
would break user preset configs and the automation API schema for no
gain. The flag stays; its meaning becomes "this feature gets the
discovery interview + plan file".

**Replace outright: the file semantics.** `ensure_plan_mode_claude_md`
is not worth preserving even as a legacy path:

- The plan file is a **single shared repo-root `PLAN.md`** for all
  features/worktrees of a project; the skeleton is only written if
  the file is missing, so a second plan-mode feature silently
  inherits the first one's plan, and concurrent features clobber
  each other. The injected block even instructs agents that "other
  agents working in parallel will read this same file" — it was
  designed for a swarm-on-one-task workflow, which contradicts
  per-feature planning.
- Instructions are injected only into `CLAUDE.local.md`, so
  codex/opencode/pi features get a PLAN.md skeleton with **no agent
  ever told about it**.
- Disabling plan mode strips the instruction block but leaves the
  repo-root `PLAN.md` (and gitignore entries) behind forever.
- The skeleton content ("Task 1") adds nothing over no file.

**Already right, keep untouched: the display layer.** The sidebar
plan preview (`read_plan_preview` → `sidebar_plan_cache`, refreshed
on prompt submit) and the markdown-viewer candidate list are
independent of the flag and already search the **workdir first**,
with `.claude/plan.md` as the top candidate before repo-root
`PLAN.md`. Moving the plan's home to per-workdir `.claude/plan.md`
therefore fixes the shared-file problem *and* makes every feature's
sidebar show its own plan, with zero display-layer changes. (Today,
all plan-mode features' sidebars show the same shared plan.)

Net: this is a gut-and-replace, not a greenfield rebuild and not a
pure extension. Epic 1 deletes the shared-PLAN.md behavior rather
than keeping it for old features — existing repo-root `PLAN.md`
files simply stop being written to and remain gitignored artifacts
the user can delete.

## Proposed design

### UX flow

```text
Feature wizard (plan_mode on) ──┐
On-demand command on a feature ─┴─> PlanInterview mode
    Phase 0  Feature brief: one free-text prompt
             "Describe the feature" (multi-line TextEditor)
    Phase 1  Static questions, one dialog per question:
             built-in bank + project question templates
             (free-text or select-options; skippable; back-nav)
    Phase 2  AI adaptive rounds (capped, e.g. 2 rounds × ≤5 questions):
             a headless run of the feature's agent harness gets
             brief + answers + repo context, returns follow-up
             questions as JSON; loading overlay while it runs;
             failure falls back to "no follow-ups"
    Phase 3  Synthesis: a headless harness run turns the full Q&A into a
             structured plan (Goal / Decisions / Architecture / UI /
             Tasks / Risks); loading overlay
    Phase 4  Review gate: plan opens in the markdown viewer with
             edit (TextEditor), accept, regenerate, and abort actions
    Accept ─> write .claude/plan.md, persist transcript,
              launch/seed session
```

Every phase supports `Esc`-out with confirmation, and a dedicated
"synthesize now" key ends the questioning early from any phase and
jumps straight to synthesis with the answers so far. Answers are
persisted as a **draft** in SQLite as they're given (decided
2026-07-13): abandoning the mode or restarting AMF keeps the draft,
and re-entering the interview for that feature offers to resume or
discard it. Accepting the plan finalizes the draft into the
transcript; drafts for deleted features are cleaned up with the
feature. (Draft persistence lands with Epic 5's table; Epics 1–4
hold the in-progress interview in memory only.)

### Question model

One shared shape for all three sources:

```rust
pub struct PlanQuestion {
    pub id: String,            // stable slug, e.g. "ui-surface"
    pub text: String,
    pub kind: PlanQuestionKind,   // FreeText | Select(Vec<String>)
    pub source: QuestionSource,   // Builtin | Template | Ai { round }
    pub optional: bool,           // skippable without an answer
}
```

- **Built-in bank** (in code, `src/plan_interview.rs`): a small
  curated set covering scope ("what's in / explicitly out?"), users
  and entry points, UI surface, data model / persistence, external
  integrations, risks/unknowns, and definition of done. Order fixed;
  all optional except the feature brief.
- **User templates** (config): a `plan_questions` array in
  `config.json`, merged global → project by `id` exactly like
  `feature_presets` / `prompt_templates` in `extension.rs` (project
  wins). Select options use the same authoring shape as `HookPrompt`
  options. A project can also set `skip_builtin_questions: true`.
- **AI-adaptive**: an interviewer prompt (constant, versioned in
  code) receives the feature brief, all prior Q&A, and cheap repo
  context (README head, top-level dir listing, CLAUDE.md if present)
  and must return `{"questions": [{id, text, kind, options?}]}`.
  Responses are parsed defensively; malformed output ⇒ skip the
  round, log to debug log, continue. Round cap and per-round question
  cap are constants (start 2 × 5).

### AI plumbing (harness-agnostic)

- New **headless runner** abstraction, `src/headless.rs`:
  `run_headless(agent: AgentKind, workdir, prompt) -> Result<String>`
  (plus a `spawn_` variant for off-thread use) dispatching per
  harness. Invocations (verified against installed CLIs 2026-07-13;
  re-verify at implementation):
  - claude: `claude -p --output-format text` with the prompt piped
    over stdin (exists today as `ClaudeLauncher::run_headless`,
    which pipes stdin to dodge the Linux argv size cap)
  - codex: `codex exec --sandbox read-only --ephemeral
    --skip-git-repo-check --color never -C <workdir> -` with the
    prompt on stdin (read-only sandbox: interview calls must not
    touch the tree; `--ephemeral` keeps interview runs out of
    session history)
  - opencode: `opencode run <prompt>` in `<workdir>` (default text
    output; `--format json` emits raw event streams, more parsing
    for no gain)
  - pi: no confirmed non-interactive mode; probe at runtime and
    fall through to the fallback order until verified
  `headless_available(agent) -> bool` probes the binary once so the
  UI can say up front which engine will power the interview. All
  harnesses take the prompt over stdin where supported — interview
  prompts carry accumulated Q&A plus repo context and can outgrow
  the Linux argv size cap (the E2BIG failure `run_headless` already
  guards against).
- **Harness selection:** prefer the feature's configured agent; if
  it has no headless support or isn't installed, fall back to the
  first available harness (claude → codex → opencode), and if none
  are available degrade to static-questions-only with a notice. The
  *implementing* session always keeps the feature's configured
  harness regardless of which engine ran the interview.
- **Structured output without provider flags:** rather than relying
  on claude's `--output-format json`, the interviewer/synthesis
  prompts instruct the model to reply with a single fenced
  ```` ```json ```` block, and the parser extracts the last fenced
  block from the reply. One prompt contract + one parsing path works
  identically across every harness; parsing stays defensive
  (malformed ⇒ skip round, log, continue).
- All headless calls run off the UI thread using the existing
  spawn-then-poll pattern (`PrReviewLoading` /
  `ReviewMemoryBootstrapRunning` are the precedents), with a
  `PlanInterviewLoading`-style frame showing the stage, engine,
  elapsed time, and — where the harness reports it — tokens used
  ("Generating follow-up questions (codex) · 12s · 3.1k tokens").
  Token figures come from the harness's own output/metadata via the
  existing usage subsystem; harnesses that don't report stay
  time-only.
- Follow-up (out of scope here): `summary.rs` hardcodes
  `ClaudeLauncher::run_headless` today even for codex/opencode/pi
  sessions; once the runner exists it should switch over.

### Synthesis output & handoff

The synthesis prompt produces markdown with a fixed skeleton:

```markdown
# Plan: <feature name>

## Goal
## Decisions            <- distilled from answers, one bullet each
## Architecture
## UI
## Tasks
- [ ] ...
## Risks / open questions
```

On **accept**:

1. Write the doc to the workdir's `.claude/plan.md` (per-feature —
   see the audit verdict; the sidebar preview and markdown viewer
   already prefer this path), ensure it's gitignored within
   `.claude/`, and inject the plan-instructions block into the file
   the feature's harness actually reads — today's code only writes
   `CLAUDE.local.md`, which codex/opencode/pi never read, so plan
   mode is silently claude-only on the instruction side. Replace
   `ensure_plan_mode_claude_md` with
   `ensure_plan_mode_instructions(workdir, agent, enabled)`:
   `CLAUDE.local.md` for claude, `AGENTS.md` for codex/opencode/pi,
   same marker-block inject/strip approach. AGENTS.md hygiene
   (decided 2026-07-13): strip the block when plan mode is disabled
   and on feature stop/delete; gitignore AGENTS.md only when AMF
   created the file from scratch (never gitignore a user's committed
   AGENTS.md). Residual risk — the block landing in a user's commit
   mid-feature — is accepted; the markers make it obvious and
   trivially removable. The block tells the agent the plan was
   authored with the user and decisions in it are settled unless the
   user says otherwise.
2. Persist the interview transcript (Q&A pairs, source of each
   question, generated plan, timestamps) to SQLite — new migration,
   `plan_interviews` table keyed by feature id — so on-demand re-runs
   can show prior answers and offer "keep / change" per question.
3. Feature-creation trigger: continue the normal launch
   (`start_feature`), then seed the agent's composer with a short
   kickoff prompt pointing at `.claude/plan.md` (editable, not
   auto-submitted — same pattern as TODO spawn's
   `open_compose_seeded`).
   On-demand trigger: just rewrite the plan file and notify; if the
   feature's session is live, offer to send the kickoff prompt.

### Triggers & integration points

- **Feature creation:** after the wizard's final step, when
  `state.plan_mode` is true, transition into
  `AppMode::PlanInterview(...)` instead of launching immediately;
  the prepared launch (`PreparedFeatureLaunch`) is carried in the
  interview state and executed on accept (abort ⇒ ask whether to
  launch anyway without a plan file, or cancel
  the feature). The dormant `CreateFeatureStep::TaskPrompt` step is
  superseded by the interview's feature brief.
- **On-demand:** command-picker entry ("Plan interview") plus a
  dashboard keybinding on the selected feature; runs the same
  `PlanInterview` mode with no pending launch. Re-running with an
  existing transcript pre-fills prior answers.
- **Presets:** `FeaturePreset.plan_mode` keeps working; a preset with
  plan mode on flows into the interview automatically. Batch feature
  creation (`CreateBatchFeaturesState`) explicitly skips the
  interview (fan-out features shouldn't each demand an interview).

### New surface area (files)

```text
src/headless.rs              # harness-agnostic headless runner:
                             # dispatch by AgentKind, availability
                             # probe, fallback order
src/plan_interview.rs        # question model, built-in bank,
                             # interviewer/synthesis prompts,
                             # fenced-JSON parsing (unit-testable,
                             # no UI)
src/app/plan_interview.rs    # App methods: enter/advance/answer/
                             # skip/back, spawn AI rounds, accept
src/handlers/plan_interview.rs  # key dispatch for the mode
src/ui/dialogs/plan_interview.rs # question dialog, progress header
                             # ("Question 4/9 · AI round 1"),
                             # loading + review-gate frames
src/db/plan_interviews.rs    # transcript persistence
```

Plus edits: `AppMode` variants (`PlanInterview`,
`PlanInterviewLoading` or an embedded stage enum), `extension.rs`
(`plan_questions` merge), `feature_ops.rs` (launch deferral),
migrations, help overlay, docs.

## Progress

Agile-inspired: epics land in order, each independently shippable and
demoable. Check items off as they merge.

### Epic 1 — Interview engine + native UI (static questions only)

Demo: plan-mode feature creation walks through the built-in bank and
writes answers verbatim into the workdir's `.claude/plan.md` under a
`## Q&A` heading (no AI yet), then launches; the sidebar preview
picks the plan up with no display-layer changes.

- [x] `src/plan_interview.rs`: `PlanQuestion` model, built-in
      question bank, unit tests
- [x] `AppMode::PlanInterview(PlanInterviewState)` with phase/step
      state machine (brief → static questions → done)
- [x] Question dialog UI: multi-line free-text (TextEditor) and
      select-options rendering, progress header, skip/back keys,
      "finish early" key (writes answers-so-far; becomes
      "synthesize now" once Epic 4 lands)
- [x] Feature-creation integration: defer `PreparedFeatureLaunch`
      until interview completes; abort path (launch-anyway vs cancel)
- [x] Write brief + answers into the workdir's `.claude/plan.md`
      (gitignored within `.claude/`)
- [x] Replace `ensure_plan_mode_claude_md` with
      `ensure_plan_mode_instructions(workdir, agent, enabled)`:
      marker block into the harness's instruction file
      (`CLAUDE.local.md` for claude, `AGENTS.md` for
      codex/opencode/pi), pointing at the per-workdir plan file —
      fixes existing plan mode being instruction-visible to claude
      only
- [x] Delete the shared repo-root `PLAN.md` skeleton/gitignore
      behavior (per audit verdict; old PLAN.md files are inert
      leftovers, not migrated)
- [ ] Unit test: non-worktree first feature (workdir == repo root)
      reads/writes the same `.claude/plan.md` the sidebar fallback
      finds — no duplicate plan files
- [ ] Help overlay + keybinding docs for the new mode

### Epic 2 — User-defined question templates

Demo: a project `config.json` adds/overrides questions; wizard
interview shows them merged with (or replacing) the built-in bank.

- [ ] `plan_questions` schema in `extension.rs` + global/project
      merge by `id` (project wins), `skip_builtin_questions` flag
- [ ] Select-option questions authored in config (HookPrompt-style)
- [ ] Config-wizard / docs coverage; `amf-add-plan-question` skill
      (optional, mirrors `amf-add-prompt`)
- [ ] Merge + parse unit tests

### Epic 3 — AI adaptive questioning

Demo: after static questions, a loading frame appears and AI
follow-ups tailored to the answers get asked — powered by the
feature's own harness (claude, codex, or opencode); failure or an
environment with no headless-capable harness silently proceeds
without them.

- [ ] `src/headless.rs`: runner dispatching by `AgentKind`
      (`claude -p`, `codex exec`, `opencode run`), availability
      probe, fallback order (feature's agent → claude → codex →
      opencode → static-only)
- [ ] Interviewer prompt constant (fenced-JSON reply contract) +
      repo-context gatherer (bounded: README head, dir listing,
      CLAUDE.md)
- [ ] Off-UI-thread spawn + poll with loading stage frame showing
      the engine in use, elapsed time, and tokens where the harness
      reports them (via the usage subsystem; time-only otherwise)
- [ ] Defensive fenced-JSON→`PlanQuestion` parsing, round/question
      caps, fallback-on-failure, debug-log breadcrumbs
- [ ] Unit tests for parsing, cap enforcement, and fallback order

### Epic 4 — Synthesis + review gate

Demo: interview ends in a generated structured plan the user can
edit, regenerate, accept (writes `.claude/plan.md`, launches session
with
seeded kickoff composer), or abort.

- [ ] Synthesis prompt constant + headless call with loading frame
- [ ] Review-gate UI: render plan via markdown viewer, `e` to edit
      raw markdown in TextEditor, `r` regenerate, `Enter` accept,
      `Esc` abort-with-confirm
- [ ] Accept path: write `.claude/plan.md`, augment the instruction
      block ("plan is user-approved"), run deferred launch, seed
      composer kickoff prompt via `open_compose_seeded`
- [ ] Replace Epic 1's raw-Q&A plan-file write with synthesized doc
      (raw Q&A kept as fallback when synthesis fails)

### Epic 5 — On-demand interviews + persistence

Demo: press the keybinding on an existing feature, re-run the
interview with prior answers pre-filled, get an updated
`.claude/plan.md`.

- [ ] Migration + `src/db/plan_interviews.rs`: interview store keyed
      by feature id (questions, answers, source, plan, timestamps,
      draft-vs-final state)
- [ ] Draft persistence: save answers as given; on interview entry
      with an existing draft, offer resume/discard; clean up drafts
      when their feature is deleted
- [ ] Save final transcript on accept (both triggers)
- [ ] Command-picker entry + dashboard keybinding for the selected
      feature; no-pending-launch variant of the mode
- [ ] Re-run flow: pre-fill prior answers, per-question keep/change
- [ ] Live-session handoff: offer to send kickoff prompt when the
      feature's agent session is running

### Epic 6 — Polish

- [ ] Preset interplay verified (preset with plan_mode → interview);
      batch creation explicitly skips with a notice
- [ ] Empty/edge handling: zero-question config, brief-only fast path
      ("just synthesize from the brief"), giant answers
- [ ] CHANGELOG + CLAUDE.md architecture-section updates
- [ ] Update this doc's status / trim to a pointer once in progress

## Open questions

Only one remains; the rest were resolved on 2026-07-13 (decisions
recorded inline in the design sections above):

- **Pi headless support:** does pi expose a usable non-interactive
  mode? Not installed on the dev box to verify. Until confirmed, pi
  features run their interview through the fallback order
  (claude → codex → opencode → static-only) via the runtime probe.

### Resolved (2026-07-13)

- **Cross-restart resume** → persist drafts to SQLite; resume/discard
  offered on re-entry (see UX flow; lands with Epic 5).
- **AGENTS.md hygiene** → marker block + strip on disable/stop/delete;
  gitignore only if AMF created the file (see handoff section).
- **Headless flags** → pinned against installed CLIs (see AI
  plumbing); re-verify at implementation.
- **Question budget UX** → yes: dedicated "synthesize now" /
  "finish early" key from any phase (Epic 1).
- **Non-worktree feature collisions** → fine by design (one workdir =
  one plan file); unit test added to Epic 1.
- **Token cost visibility** → shown in the loading frame (engine,
  elapsed, tokens where reported); wired via the usage subsystem
  (Epic 3).

## Reasoning / when to build

Plan mode is the natural differentiator for AMF's "manage many
agents" story: the cheapest way to raise multi-agent output quality
is better up-front specification, and AMF owns the moment right
before a session launches. The design deliberately reuses existing
machinery (HookPrompt-style dialogs, extension-config merging,
existing headless launch code, markdown viewer, compose seeding) so
most epics are integration work rather than new subsystems; the one
genuinely new piece, the harness-agnostic headless runner, also pays
for itself outside this feature (session summaries currently
hardcode claude). Epic 1 alone already
improves on the status quo (structured intake instead of a skeleton
file) and everything after it layers on without breaking the boolean
plan mode that presets and existing features rely on.
