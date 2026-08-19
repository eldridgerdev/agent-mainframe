# Plan Mode: guided feature discovery interview

- **Status:** Complete
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
   root-level `AMF_PLAN.md` (superseded on 2026-08-19 from the
   `.claude/plan.md` location chosen in the 2026-07-13 audit below).

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

## Plan location follow-up (2026-08-19)

The July audit correctly rejected one `PLAN.md` shared from the project
repository by every worktree, but its replacement remained unnecessarily
Claude-specific. A feature worktree's own root is already isolated from every
other feature, so its plan is per-feature without living under
`.claude/`. The first non-worktree feature also remains safe because AMF allows
only one non-worktree feature per project.

The accepted plan therefore now lives at `<workdir>/AMF_PLAN.md`, with a
matching root `.gitignore` entry. The AMF-specific name avoids overwriting a
repository's conventional `PLAN.md`. The sidebar and markdown viewer prefer
the namespaced file while retaining `PLAN.md` and `.claude/plan.md` as
fallbacks. The approved-plan kickoff is also carried through the common launch
payload, so Codex receives the same editable, unsubmitted composer seed as
Claude even when startup steering is enabled. Existing files are not deleted
automatically.

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
    Phase 2  Optional AI adaptive rounds (capped, e.g. 2 rounds × ≤5
             questions): an explicit token-use prompt offers three
             deliberately distinct outcomes:
             a       ask AI to generate more interview questions,
                     then answer them before plan drafting
             Ctrl+F  stop asking questions and use AI to draft the
                     plan now from answers already collected
             Enter   make no AI call and review the raw interview plan
             The feature's agent harness receives the brief + answers
             + repo context; failure falls back to "no follow-ups"
    Phase 3  Synthesis: a headless harness run turns the full Q&A into a
             structured plan (Goal / Decisions / Architecture / UI /
             Tasks / Risks); loading overlay
    Phase 4  Review gate: plan opens in the markdown viewer with
             edit (TextEditor), accept, regenerate, and abort actions
    Accept ─> write AMF_PLAN.md, persist transcript,
              launch/seed session
```

Every phase supports `Esc`-out with confirmation. “Ask AI follow-ups”
and “draft the plan now” are not synonyms: the first creates more
questions for the user to answer, while the second ends questioning
and generates the plan immediately from the answers already saved.
Interview answers are persisted as a **draft** in SQLite as they're
given (decided
2026-07-13): abandoning the mode or restarting AMF keeps the draft,
and re-entering the interview for that feature offers to resume or
discard it. Accepting the plan finalizes the draft into the
transcript; drafts for deleted features are cleaned up with the
feature. Because the feature-creation trigger runs before the feature
exists, its draft is keyed by project + branch until the accept re-files
the transcript under the real feature id.

### Question model

One shared shape for all three sources:

```rust
pub struct PlanQuestion {
    pub id: String,            // stable slug, e.g. "ui-surface"
    pub text: String,
    pub kind: PlanQuestionKind,   // FreeText | Select(Vec<String>)
    pub source: QuestionSource,   // Builtin | GlobalTemplate | Template | Ai { round }
    pub optional: bool,           // skippable without an answer
}
```

- **Built-in bank** (in code, `src/plan_interview.rs`): five curated
  prompts covering scope; users, entry points, and workflow/UI changes;
  data, persistence, and external integrations; risks/unknowns; and
  definition of done. The two related groups were consolidated after
  dogfooding so the default interview stays short. Order fixed; all
  optional except the feature brief.
- **User templates** (config): a `plan_questions` array in
  `config.json`, merged global → project by `id` exactly like
  `feature_presets` / `prompt_templates` in `extension.rs` (project
  wins after trimming surrounding ID whitespace). Select options use
  the same authoring shape as `HookPrompt` options. A project can also
  set `skip_builtin_questions: true`. The interview progress header
  identifies configured questions as global or project templates.
- **AI-adaptive**: an interviewer prompt (constant, versioned in
  code) receives the feature brief, all prior Q&A, and cheap repo
  context (README head, top-level dir listing, CLAUDE.md if present)
  and must return `{"questions": [{id, text, kind, options?}]}`.
  Before the first call, an explicit consent step explains that
  adaptive rounds use agent tokens; declining completes the
  interview without any headless request.
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
  - pi: `pi -p --no-session`, with the prompt piped over stdin;
    context-complete calls add `--no-tools` plus resource-discovery
    shutoffs, while repository investigations allow only
    `read,grep,find,ls`
  `headless_available(agent) -> bool` probes the binary once so the
  UI can say up front which engine will power the interview. All
  harnesses take the prompt over stdin where supported — interview
  prompts carry accumulated Q&A plus repo context and can outgrow
  the Linux argv size cap (the E2BIG failure `run_headless` already
  guards against).
- **Harness selection:** prefer the feature's configured agent (including Pi
  when its installed CLI advertises the full safe-headless flag set); if
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
- **Interview prompts must state that the model has no tools.** These
  calls run restricted (no file access), so a prompt that asks the
  model to "ground this in the repository" invites it to answer with
  an offer to go read the repo and nothing else — observed live during
  the Epic 4 agent-review work, where that reply was the entire
  response. All three prompts (interviewer, synthesis, critique) now say
  they run without tools and that the supplied context is all there is.
  A unit test asserts the line on each and then scans the module source
  for `pub const *_PROMPT` declarations, so a fourth prompt fails the
  test rather than silently escaping a hand-written list. Every
  parse-failure path logs a bounded ~300-char prefix of the reply; that
  breadcrumb is what makes this class of failure diagnosable at all.
- All headless calls run off the UI thread using the existing
  spawn-then-poll pattern (`PrReviewLoading` /
  `ReviewMemoryBootstrapRunning` are the precedents), with a
  `PlanInterviewLoading`-style frame showing the stage, engine,
  elapsed time, and — where the harness reports it — tokens used
  ("Generating follow-up questions (codex) · 12s · 3.1k tokens").
  Token figures come from the harness's own output/metadata via the
  existing usage subsystem; harnesses that don't report stay
  time-only.
- Completed follow-up (2026-08-06): `summary.rs` now uses the shared
  `HeadlessRunner` with the feature's configured harness, so codex, opencode,
  and pi session summaries no longer invoke Claude. Summary generation is a
  restricted no-tools call because the captured pane already supplies all of
  the context it needs.

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

1. Write the doc to the workdir's root-level `AMF_PLAN.md` (per-feature —
   see the 2026-08-19 follow-up; the sidebar preview and markdown viewer
   prefer this path), ensure it's gitignored in the workdir root, and
   inject the plan-instructions block into the file
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
   kickoff prompt pointing at `AMF_PLAN.md` (editable, not
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
writes answers verbatim into the workdir's `AMF_PLAN.md` under a
`## Q&A` heading (no AI yet), then launches; the sidebar preview
picks the plan up with no display-layer changes.

- [x] `src/plan_interview.rs`: `PlanQuestion` model, built-in
      question bank, unit tests
- [x] `AppMode::PlanInterview(PlanInterviewState)` with phase/step
      state machine (brief → static questions → done)
- [x] Question dialog UI: multi-line free-text (TextEditor) and
      select-options rendering, progress header, skip/back keys,
      "finish early" key (writes answers-so-far; becomes
      "draft plan now" once Epic 4 lands)
- [x] Feature-creation integration: defer `PreparedFeatureLaunch`
      until interview completes; abort path (launch-anyway vs cancel)
- [x] Write brief + answers into the workdir's `AMF_PLAN.md`
      (gitignored in the workdir root)
- [x] Replace `ensure_plan_mode_claude_md` with
      `ensure_plan_mode_instructions(workdir, agent, enabled)`:
      marker block into the harness's instruction file
      (`CLAUDE.local.md` for claude, `AGENTS.md` for
      codex/opencode/pi), pointing at the per-workdir plan file —
      fixes existing plan mode being instruction-visible to claude
      only
- [x] Delete the shared project-repo `PLAN.md` skeleton behavior
      (per audit verdict); the accepted plan is instead written to the
      feature workdir's own root after the interview
- [x] Unit test: non-worktree first feature (workdir == repo root)
      reads/writes the same `AMF_PLAN.md` the sidebar
      finds — no duplicate plan files
- [x] Help overlay + keybinding docs for the new mode

### Epic 2 — User-defined question templates

Demo: a project `config.json` adds/overrides questions; wizard
interview shows them merged with (or replacing) the built-in bank.

- [x] `plan_questions` schema in `extension.rs` + global/project
      merge by `id` (project wins), `skip_builtin_questions` flag
- [x] Select-option questions authored in config (HookPrompt-style)
- [x] Normalized ID overrides and accurate Global/Project template labels
- [x] Config-wizard / docs coverage (`amf-add-plan-question` remains an
      optional follow-up, mirroring `amf-add-prompt`)
- [x] Merge + parse unit tests

### Epic 3 — AI adaptive questioning

Demo: after static questions, an explicit token-use prompt offers AI
follow-ups. Opting in opens a loading frame and asks questions
tailored to the answers — powered by the feature's own harness
(claude, codex, opencode, or pi); declining, failure, or an environment
with no headless-capable harness silently proceeds without them.

- [x] `src/headless.rs`: runner dispatching by `AgentKind`
      (`claude -p`, `codex exec`, `opencode run`, `pi -p`), availability
      probe, fallback order (feature's agent → claude → codex →
      opencode → static-only). Pi was enabled on 2026-08-06 after its
      print, ephemeral-session, no-tools, read-only tool allowlist, resource
      isolation, project-trust, and model flags were verified; older Pi
      versions continue through the fallback order. Visual proof:
      `docs/screenshots/plan-mode-pi-headless/`, regenerable via
      `scripts/dev/screenshot/scenarios/plan-interview-pi-headless.txt`
- [x] Interviewer prompt constant (fenced-JSON reply contract) +
      repo-context gatherer (bounded: README head, dir listing,
      CLAUDE.md)
- [x] Explicit opt-in gate before any adaptive headless call;
      declining or finishing completes with zero adaptive token use
- [x] Off-UI-thread spawn + poll with loading stage frame showing
      the engine in use, elapsed time, and tokens where the harness
      reports them (via the usage subsystem; time-only otherwise) —
      landed as a cheap prompt-size token estimate
      (`app::pr_review::estimate_tokens`, reused), not a real
      harness-reported count; no headless call currently surfaces
      actual usage
- [x] Defensive fenced-JSON→`PlanQuestion` parsing, round/question
      caps, fallback-on-failure, debug-log breadcrumbs
- [x] Unit tests for parsing, cap enforcement, and fallback order

### Epic 4 — Synthesis + review gate

Demo: interview ends in a generated structured plan the user can
edit, regenerate, accept (writes `AMF_PLAN.md`, launches session
with
seeded kickoff composer), or abort.

- [x] Synthesis prompt constant + headless call with loading frame
- [x] Review-gate UI: render plan via markdown viewer, `e` to edit
      raw markdown in TextEditor, `r` regenerate, `Enter` accept,
      `Esc` abort-with-confirm (`Ctrl+S` saves an edit back to preview;
      `Esc` from the editor discards the edit)
- [x] Optional agent-review action from the review gate: have an agent
      inspect the draft plan and provide a detailed analysis of gaps,
      risks, contradictions, unclear decisions, and missing acceptance
      criteria; present the analysis as advisory feedback without
      changing the plan unless the user chooses to revise it — `a` at the
      review gate runs `CRITIQUE_PROMPT` through the same headless engine
      and opens the analysis in the markdown viewer (`Esc`/`Enter` back to
      the untouched plan, `r` revises). Revision reuses the synthesis pass
      with the review attached as `reviewer_feedback`; a stale review is
      dropped whenever the plan changes. `Esc` during the review returns
      to the plan rather than opening the interview's abort confirmation,
      so a generated plan can't be lost to a stray keypress
- [x] Accept path: write `AMF_PLAN.md`, augment the instruction
      block ("plan is user-approved"), run deferred launch, seed
      composer kickoff prompt via `open_compose_seeded` — the launch's
      `ensure_feature_running` already injects the approved-plan block,
      so accept adds the seeding step and lands the user in the new
      session's composer with an editable, unsubmitted kickoff prompt.
      Best-effort by design: it runs after the feature is created and
      started, so a feature with no tmux-backed agent session skips the
      seed rather than failing the accept. The kickoff uses the common
      startup-prompt payload and takes precedence over a blank startup
      steering prompt, which keeps Codex and Claude behavior aligned
- [x] Replace Epic 1's raw-Q&A plan-file write with synthesized doc
      (raw Q&A kept as fallback when synthesis fails)
- [x] Omit skipped and unanswered questions from both synthesis input
      and the raw-Q&A fallback plan so they do not add irrelevant context
      — blank answers count as skips, and an interview with nothing
      answered degrades to the brief alone (no empty `## Q&A`). The
      interviewer and critique prompts deliberately still receive the
      full asked-set with `answer: null`: the interviewer must not
      re-ask what the user passed over, and the reviewer judges the plan
      against everything the interview covered

### Epic 5 — On-demand interviews + persistence

Demo: press the keybinding on an existing feature, re-run the
interview with prior answers pre-filled, get an updated
`AMF_PLAN.md`.

- [x] Migration + `src/db/plan_interviews.rs`: interview store keyed
      by feature id (questions, answers, source, plan, timestamps,
      draft-vs-final state) — `MIGRATION_016` keys on
      `(feature_id, stage)` rather than `feature_id` alone so a re-run
      can save a draft without destroying the accepted transcript it is
      revising; `finalize_draft` promotes one to the other in a single
      transaction. `questions`/`answers` are JSON columns (read and
      written whole, and `PlanQuestion` already serializes), padded to
      equal length on both save and load so every reader gets an aligned
      pair; `answer_for(id)` is the id-keyed lookup the re-run pre-fill
      needs when config has changed the bank between runs.
      `ai_rounds_completed` is stored rather than derived from the
      question list: a round that returned nothing usable still counted
      against the cap, and resuming a draft must not hand back paid
      rounds. Like `todo_lists`, `feature_id` carries no FK —
      `store::save` full-replaces `features` and would cascade-wipe the
      rows (covered by a test) — so cleanup is explicit via
      `delete_for_feature`, wired into the feature-delete path with the
      next item
- [x] Draft persistence: save answers as given; on interview entry
      with an existing draft, offer resume/discard; clean up drafts
      when their feature is deleted — the draft is saved after every
      action that records something (advance, skip, back, finish-early,
      a finished AI round, a synthesized plan, a plan edit) and skipped
      until the brief exists, since an interview with no brief has
      nothing to resume into. A feature-creation interview predates the
      feature's uuid, so its draft is keyed
      `pending:<project>/<branch>` (`plan_interview::pending_interview_key`)
      — the identity the user re-enters when they come back to create the
      same feature. `PlanInterviewPhase::ResumePrompt` is the first screen
      when a draft exists: `r` resumes, `d` discards (deleting the row),
      `Esc` keeps it and falls through to the normal abort choice. Resume
      matches answers by **question id**, not position, because config can
      change the bank between runs; stored AI questions absent from the
      current bank are appended and `ai_rounds_completed` restored, so
      paid rounds are never re-earned; a draft that already holds a
      generated plan reopens at the review gate instead of synthesizing
      again. Persistence is silent on failure throughout — the interview
      runs entirely from memory without a DB (covered by a test). Visual
      proof: `docs/screenshots/plan-mode-draft-resume/`, regenerable via
      `scripts/dev/screenshot/scenarios/plan-interview-resume.txt`
- [x] Save final transcript on accept (both triggers) — landed with the
      draft lifecycle rather than after it: a draft still offered for
      resume after a successful accept is a bug in the item above, and
      consuming it via `finalize_draft` is the same work as deleting it.
      `finalize_draft` now takes both keys and re-files the transcript
      under the feature id the accept just created, which is where the
      re-run pre-fill will look for it
- [x] Command-picker entry + dashboard keybinding for the selected
      feature; no-pending-launch variant of the mode — `P` on the
      dashboard and a `plan-interview` command-picker entry (offered only
      with a feature or session selected, since the interview plans one
      workdir) both open
      `PlanInterviewState::for_feature`, keyed by the feature's **id** so
      the accepted transcript lands where the re-run pre-fill will look
      for it. The workdir moved out of `pending_launch` onto the state
      itself (`context_workdir()` backs the three headless call sites,
      which previously fell back to the process cwd), so accept writes the
      plan into the feature's own workdir rather than bailing out. Accept
      also calls `ensure_plan_mode_instructions` and sets
      `feature.plan_mode`: writing `AMF_PLAN.md` alone leaves the agent
      never told the plan exists, and a restart would stop injecting the
      block. Abort is non-destructive — there is no launch to cancel, so
      the confirm offers only "leave the plan unchanged" and `n` is inert.
      Visual proof: `docs/screenshots/plan-mode-on-demand/`, regenerable via
      `scripts/dev/screenshot/scenarios/plan-interview-on-demand.txt`. The
      capture stops short of accepting — that runs a real headless synthesis
      call, so the accept path is covered by
      `accepting_an_on_demand_plan_writes_it_into_the_features_own_workdir`
      instead
- [x] Re-run flow: pre-fill prior answers, per-question keep/change — entering
      an on-demand interview loads the feature's accepted transcript
      (`plan_interview_final`) and adopts it as the run's **baseline**: the brief
      lands in the editor and every answer is matched back by question **id**
      (config can change the bank between runs), with the previous run's AI
      questions carried in with their answers since the current bank cannot
      contain them. Spent AI rounds are deliberately *not* carried — a re-run is
      a new interview and gets its own consent and its own budget. Keep/change is
      in-place rather than a per-question card: `Enter` keeps the pre-filled
      answer, typing changes it, and `Ctrl+R` restores the accepted one, with a
      note under the question naming which of the three states it is in
      (kept / changed / cleared). The note carries the `Ctrl+R` hint because the
      footer's hint row already wraps to both its lines at ordinary widths.
      Emptying a pre-filled answer records as a skip, which is how a re-run drops
      an answer that no longer applies. A draft and an accepted transcript can
      both exist for one feature: the draft prompt still comes first and its
      answers win when resumed, but discarding it now falls back to the accepted
      answers instead of blanking the interview. Visual proof:
      `docs/screenshots/plan-mode-rerun/`, regenerable via
      `scripts/dev/screenshot/scenarios/plan-interview-rerun.txt` — a re-run needs
      an accepted transcript to read, so the capture inserts that row into the
      scratch DB directly rather than paying for a real synthesis pass (snippet in
      that directory's README)
- [x] Live-session handoff: offer to send kickoff prompt when the
      feature's agent session is running — an accepted **on-demand** plan lands
      `PlanInterviewPhase::KickoffHandoff` when the feature has a live
      agent-harness session, offering to open it with the composer seeded
      (`y`) or leave it running (`n`/`Esc`). The offer comes strictly *after*
      the accept has fully landed — plan file, instruction block, `plan_mode`,
      transcript — so declining costs nothing and the only thing on offer is
      interrupting a session that may be mid-task; the seed is editable and
      unsubmitted like every other compose seed. "Live" means both halves,
      feature not `Stopped` **and** `session_exists`: status is only reconciled
      every few seconds, so a session killed outside AMF still reads as running
      and would otherwise be offered a prompt into nothing. The target is held
      by session **id**, not index, because the accept saves the store before
      the prompt is answered. The feature-creation trigger is unchanged — it
      seeds the session it just launched without asking, since there is no
      running work to interrupt. Visual proof:
      `docs/screenshots/plan-mode-live-handoff/`, regenerable via
      `scripts/dev/screenshot/scenarios/plan-interview-live-handoff.txt` —
      reaching a handoff needs a real accept, so the scenario seeds a
      draft that already holds a plan
      (`scripts/dev/screenshot/seed_plan_draft.py`) and resumes it straight to
      the review gate rather than paying for a synthesis pass

### Epic 6 — Polish

- [x] Add directed feedback at the plan review gate: `f` opens a multi-line
      instruction editor and `Ctrl+S` asks the planning agent to revise the
      current draft. The revision prompt carries the plan and full interview
      transcript, runs in the feature workdir with repository-inspection tools
      constrained to read-only access, and requires the complete structured
      plan back rather than a prose report. A valid result replaces only the
      staged draft and returns to the review gate for inspection before accept;
      a failed or malformed result preserves both the prior plan and the user's
      instruction for retry. Esc keeps the plan unchanged, and a result that
      arrives after the user dismisses the loading frame is discarded rather
      than applied late. Visual proof:
      `docs/screenshots/plan-mode-directed-feedback/`, regenerable via
      `scripts/dev/screenshot/scenarios/plan-interview-directed-feedback.txt`
- [x] Add an optional isolated investigation pass after plan generation.
      From the review gate, let the user identify questions or plan sections
      that need more research, then delegate that work through the selected
      harness's subagent mechanism (Claude subagents, the Codex equivalent, or
      a separate ephemeral headless run when no native mechanism is exposed).
      Give each investigator a focused prompt and read-only repository access
      in its own context window, return only its findings to the planning
      workflow, and merge those findings into the draft for user review. This
      keeps codebase exploration from consuming the larger planning or
      implementation session's context window — `i` at the review gate now
      opens a multi-line research-focus editor; blank-line-separated focuses
      (capped at four) each run through a fresh read-only headless invocation
      using the interview's selected harness. A separate restricted invocation
      receives only the validated, size-bounded findings and merges them into
      the complete draft, so provider tool traces and exploration context never
      enter the planning or implementation session. This portable isolated-run
      fallback is used instead of assuming provider-specific subagent APIs.
      Failed or dismissed passes preserve the current plan (and failures keep
      the research request for retry); late results cannot overwrite it.
      Visual proof: `docs/screenshots/plan-mode-isolated-investigation/`,
      regenerable via
      `scripts/dev/screenshot/scenarios/plan-interview-isolated-investigation.txt`
- [x] Replace the ambiguous AI-consent labels everywhere they appear
      (dialog, status footer, help, and screenshots): `a` should say
      "ask AI follow-ups" because it generates more questions;
      `Ctrl+F` should say "draft plan now" because it skips every
      remaining question and generates the plan from answers already
      collected. Keep the no-token `Enter` action explicit as
      "review raw plan." The consent dialog now spells out the behavioral
      and token-cost difference between all three actions, and the compact
      footers use those same labels. The two adaptive-interview screenshots
      were regenerated from the updated UI.
- [x] Re-evaluate the built-in question bank after dogfooding:
      reduced the default from seven questions to five by combining
      users/entry-points with workflow/UI changes and combining
      data/persistence with external integrations. Scope, risks/unknowns,
      and definition of done remain separate because each consistently
      contributes a distinct planning decision. The surviving question
      IDs remain stable; projects that override either retired ID still
      get that configured question appended as a project-specific prompt
- [x] Preset interplay verified (preset with plan_mode → interview);
      batch creation explicitly skips with a notice — an end-to-end
      feature-creation regression applies a plan-mode preset and proves the
      launch is deferred into `PlanInterview`, while the batch settings dialog
      tells users that plan interviews are skipped for batch creation (with a
      render test keeping the notice visible). Visual proof:
      `docs/screenshots/plan-mode-preset-batch/`, regenerable via
      `scripts/dev/screenshot/scenarios/plan-mode-preset-batch.txt`
- [x] Empty/edge handling: zero-question config, brief-only fast path
      ("just synthesize from the brief"), giant answers — opting out of the
      built-in bank with no configured replacements now has explicit regression
      coverage, and `Ctrl+F` from the required brief is covered end to end as a
      direct synthesis path with no question or adaptive round. Briefs and
      answers larger than 12,000 Unicode characters are preserved losslessly in
      the draft/transcript and raw-plan fallback, while each field is bounded
      with an explicit truncation marker at the model-prompt boundary so pasted
      logs or documents cannot consume an unbounded headless context. Visual
      proof of the zero-question and brief-only flow:
      `docs/screenshots/plan-mode-edge-handling/`, regenerable via
      `scripts/dev/screenshot/scenarios/plan-interview-edge-handling.txt`
- [x] CHANGELOG + CLAUDE.md architecture-section updates
- [x] Update this doc's status / trim to a pointer once in progress — status is
      now complete and the backlog index points to the shipped workflow. This
      document remains as the design/implementation record because it captures
      the settled decisions, fallback semantics, and reproducible visual proof;
      the user-facing workflow and keys live in `README.md`

### Post-ship follow-ups

- [x] Route dashboard and in-session one-line summaries through the
      harness-agnostic headless runner using the feature's configured agent,
      rather than hardcoding Claude for every feature. Visual proof:
      `docs/screenshots/harness-aware-session-summary/`, regenerable via
      `scripts/dev/screenshot/scenarios/harness-aware-session-summary.txt`
- [x] Make the mouse wheel scroll the review gate. The scroll handlers matched
      a chain of modes and then fell through to dashboard selection movement,
      with no case for the interview — so a wheel notch over a plan taller than
      the screen silently moved the selection hidden behind the dialog. The
      wheel now moves whichever pane the current phase renders (the plan, the
      agent's review, or the plan/instruction editors) through a single
      phase-to-offset accessor on the interview state; phases with nothing to
      scroll swallow the event rather than falling through. Clicks inside the
      dialog are inert for the same reason — the interview was missing from
      `handle_click`'s dialog list, so a double-click could reach a feature row
      underneath and start it. Visual proof:
      `docs/screenshots/plan-review-mouse-scroll/`, regenerable via
      `scripts/dev/screenshot/scenarios/plan-review-mouse-scroll.txt` — which
      is also the first scenario to drive mouse input, injecting SGR wheel
      bytes through the grammar's `run:` escape hatch

## Open questions

None. The original design questions are recorded below with their resolutions.

### Resolved

- **Pi headless support** → resolved 2026-08-06 from the official CLI
  contract: `-p` consumes piped stdin and exits, `--no-session` keeps calls
  ephemeral, `--no-tools` disables all tools, and
  `--tools read,grep,find,ls` provides the read-only investigation surface.
  AMF also disables discovered extensions, skills, prompt templates, and
  context files and ignores project-local config for these runs. The runtime
  availability probe requires every relied-on flag, preserving the former
  fallback behavior for older Pi installations.

- **Cross-restart resume** → persist drafts to SQLite; resume/discard
  offered on re-entry (see UX flow; lands with Epic 5).
- **AGENTS.md hygiene** → marker block + strip on disable/stop/delete;
  gitignore only if AMF created the file (see handoff section).
- **Headless flags** → pinned against installed CLIs (see AI
  plumbing); re-verify at implementation.
- **Question budget UX** → yes: dedicated "draft plan now" /
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
