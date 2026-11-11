# Plan interview and Quick Plan

Implementation notes for agents and contributors. Source is authoritative
when it disagrees; update this file in the same change.

## Plan-interview reference docs

The interview's `round` / `synthesis` / `critique` passes run **no-tools** by
default. Attaching one or more reference documents on the brief step is the
explicit, opt-in exception: those passes then run through
`HeadlessRunner::run_read_only` so the interviewer can read the attached docs
*and* the surrounding codebase.

- **Attaching:** `Ctrl+D` on the brief step opens
  `AppMode::PlanInterviewAttachDoc` — a `ratatui_explorer::FileExplorer` browser
  (`app/plan_interview_attach.rs`, `handlers/plan_interview_attach.rs`,
  `ui/dialogs/plan_interview_attach.rs`) that stashes the live
  `PlanInterviewState` and restores it on confirm or cancel, a deliberate
  sibling of `BrowsingPath` rather than a refactor of it. `Ctrl+X` drops the
  last attachment. Cap `MAX_ATTACHED_DOCS` (4). Any readable text file anywhere
  on disk; `plan_interview::validate_attachment` rejects a directory, an
  oversize file (`ATTACHED_DOC_MAX_BYTES`), a binary sniff, a duplicate, or the
  cap with a stated reason.
- **Reachability:** `plan_interview::prepare_attached_docs(workdir, &[PathBuf])`
  runs right before each pass. An in-workdir doc is referenced where it lies; an
  external one is copied into a **per-pass** subdirectory
  `<workdir>/.amf/interview-docs/<pid>-<seq>/` (generated scratch via
  `extension::generated_amf_subdir`) so a CWD-scoped read-only harness can open
  it. Each call gets its own subdir on purpose: a dismissed plan review leaves
  its worker running, and a later pass (or teardown) must not delete the copies
  it is still reading. Before the first external copy, `ensure_amf_ignored`
  adds `.amf/` to the repo's `.git/info/exclude` (untracked, per-repo) unless it
  is already ignored, so an agent's `git add -A` cannot commit a private doc.
  The whole `interview-docs` tree is cleared by
  `App::clear_plan_interview_doc_staging` on interview accept / abort **and on
  pause** (whose `background_running` guard means no pass is in flight); it is
  no longer wiped at the start of a pass. A doc that has moved or gone
  unreadable is dropped and reported, never fatal.
- **Prompts:** the five interview input builders (`*_input_json`) carry an
  `attached_documents` array (`path` + `origin`); `round` / `synthesis` /
  `critique` templates gained a `{{tool_access_note}}` token, resolved to
  `TOOL_ACCESS_NOTE_NONE` / `CRITIQUE_TOOL_ACCESS_NOTE_NONE` (historical
  no-tools wording) or `TOOL_ACCESS_NOTE_ATTACHED` (the read-only exception).
- **Persistence:** `PlanInterviewRecord.attached_docs: Vec<String>`,
  `MIGRATION_035` (column backfilled `'[]'`). A resumed or re-run interview
  restores the list verbatim; paths are re-validated at dispatch, not on
  resume.

## Plan-interview MCP tools

- **Opt-in, global only:** `AppConfig::plan_interview_mcp`
  (`headless::HeadlessMcpConfig { config?, allowed_tools }`), never
  `amf.json` — MCP servers run programs. `HeadlessMcpConfig::validate`
  requires exact `mcp__<server>__<tool>` names (no wildcards) and, when
  `config` is set, an absolute (`~/`-expanded) file. `App::plan_interview_mcp`
  resolves it per pass, returning `None` for non-Claude harnesses and for an
  invalid config (logged + toast once per distinct error, never fatal).
- **Command:** `HeadlessRunner::run_read_only_with_mcp` →
  `claude_mcp_read_only_args`. Verified against Claude Code 2.1.282: neither
  `--safe-mode` nor `--strict-mcp-config` can be used, since both drop
  claude.ai connectors (the common case — e.g. `mcp__claude_ai_Asana__*`).
  Isolation is `--setting-sources ""` (verified: a repo hook, repo
  `.mcp.json`, `CLAUDE.md`, `.claude/skills/`, and `.claude/agents/` do not
  load; repo files still reach the model as data via `Read` and the prompt's
  `repository_context`). `--tools` does not gate MCP tools, so `dontAsk` +
  the exact `--allowedTools` list is what denies unlisted ones.
  `claude_mcp_args_are_read_only` is the positive check.
- **`MCP_CONNECTION_NONBLOCKING=false`** (`CLAUDE_MCP_BLOCKING_ENV`) makes the
  run wait for claude.ai connectors; without it about half of `-p` runs
  started with no connector tools. Undocumented upstream — re-verify on a
  Claude Code upgrade.
- **Passes:** round, synthesis (full + quick), critique, critique follow-up —
  the ones with `{{tool_access_note}}`, which becomes
  `plan_interview::mcp_tool_access_note`. Directed revision and investigation
  are unchanged.

## Quick Plan mode

A lighter, dynamically-sized alternative to the full plan-mode interview: a
task-triage round that may ask a couple of clarifying questions — or none at
all for a trivial task — before deciding whether to proceed straight to work,
show a short plan, or escalate into the full interview.

- **Two triggers, same two shapes as full Plan mode:** the feature-creation
  wizard's Plan field (`Mode` step, `mode_focus == 3`) and the dashboard's `Q`
  (re-run on the selected feature, parallel to `P`). `Q` mirrors `P`'s exact
  arming: `resume_paused_plan_interview()` first, so a parked interview of
  either kind resumes rather than starting a second one
  (`handlers/normal.rs`).
- **One `PlanInterviewState`, a `kind` flag.** `PlanInterviewMode::{Full,
  Quick}` (`app/state.rs`) — the two share every `PlanInterviewPhase`, the
  round/synthesis dispatch, and the entire Q&A UI; `kind` only steers which
  `PromptId` is resolved and how the synthesis response is interpreted. A
  Quick interview is built with an **empty static question bank**
  (`PlanInterviewState::for_feature_creation_quick` /
  `for_feature_quick`), so `advance()`'s existing empty-bank handling — Brief
  goes straight to `AiConsent`, already true for a project with zero
  configured plan questions — is what skips Quick Plan past the static
  question flow with no new phase-machine code.
- **Prompts:** `PromptId::PlanInterviewQuickRound` /
  `PlanInterviewQuickSynthesis` (`plan_interview.quick_round` /
  `.quick_synthesis`), registered like any other prompt (editable via `E`).
  The round prompt reuses the full round's exact `{"questions":[...]}`
  contract and parser (`parse_ai_questions`) — only the framing differs,
  tuned to prefer zero questions for a clear task. `MAX_QUICK_AI_ROUNDS` (1,
  vs `MAX_AI_ROUNDS` for full) caps a live Quick interview to one adaptive
  round before synthesis must decide; `PlanInterviewState::max_ai_rounds()`
  is the one place that reads either constant, keyed off `kind`.
- **Synthesis is a 3-way outcome, not a markdown document.** The quick
  synthesis prompt returns one fenced ```json block:
  `{"outcome":"direct","summary":"..."}` (trivial — proceed with no plan
  artifact), `{"outcome":"plan","plan":"<markdown>"}` (the nested markdown
  follows the exact full-mode plan contract, so it drops into the same
  `AMF_PLAN.md` / Review-gate path unchanged), or
  `{"outcome":"escalate","reason":"..."}`. Parsed by
  `plan_interview::parse_quick_synthesis_outcome` into `QuickSynthesisOutcome`
  (`Direct`/`Plan`/`Escalate`/`Unparseable`); `Unparseable` — and a harness
  failure — fall back to the same raw-Q&A plan full mode uses on a failed
  synthesis. Dispatched from `App::apply_quick_synthesis_result`
  (`app/plan_interview.rs`), the one kind-aware branch inside
  `poll_plan_interview_synthesis_bg`.
- **`direct` outcome:** `App::complete_quick_plan_direct` — a
  feature-creation interview launches the deferred feature exactly like
  declining to plan at all (`prepared.plan_mode = false`, then
  `finish_feature_launch_without_interview`, the same path
  `launch_plan_interview_without_plan` uses); an on-demand interview on an
  existing feature has nothing to launch, so it just closes. Either way the
  model's optional `summary` becomes the toast message.
- **`escalate` outcome:** `App::escalate_quick_plan_to_full` flips a live
  interview's `kind` to `Full` **in place** — nothing is reset, so the brief
  and every answer already given carry forward into the full round exactly as
  the interview decision requires — sets `ai_followups_opted_in`, clears
  `synthesis_attempted` (the Quick pass was triage and left no plan; left
  set, the full rounds would end at the raw Q&A review), and
  re-enters `continue_plan_interview_after_done()` so the ordinary full-mode
  round/synthesis branching takes over from there with the full `PromptId`s.
  The user sees an explicit toast ("Quick Plan → full Plan interview: …")
  naming the model's stated reason.
- **No `plan_interviews` DB persistence for Quick, by design (v1 scope
  cut).** `persist_plan_interview_draft` no-ops for `kind == Quick` — every
  round/synthesis call site's periodic save is a safe no-op rather than
  needing its own guard — so there is no saved draft to resume and no
  TODO-origin brief prefill on entry (unlike `start_plan_interview`,
  `start_quick_plan_interview` skips both). Once escalated, `kind` is `Full`
  and persistence resumes normally for the rest of the interview.
  `feature.plan_mode` (the persisted, DB-backed "this feature has a plan to
  read" flag `ensure_plan_mode_instructions` acts on) needs no Quick sibling:
  it is set from whichever interview actually produced a plan, kind-agnostic.
- **Feature-creation Plan field is a 3-way cycle, not two booleans.**
  `CreateFeatureState::quick_plan: bool` sits alongside the existing
  `plan_mode: bool` (also `PreparedFeatureLaunch`, `FeaturePreset`) rather
  than replacing it with an enum — `plan_mode` is also the persisted
  `Feature`/DB/`FeaturePreset` field, and an enum there would ripple into
  schema and the automation JSON API this feature doesn't need to touch.
  `CreateFeatureState::cycle_plan_choice(forward)` steps
  `(plan_mode, quick_plan)` through `(false,false) → (true,true) →
  (true,false) → (false,false)` (None → Quick Plan → Full Plan → None),
  bound to Up/Down at the Plan field like every other Mode-step control.
  **Not** threaded through the `on_worktree_created` hook continuation
  (`app/hooks.rs`) in v1: a feature created through that path with Quick Plan
  chosen falls back to full Plan mode rather than losing planning entirely —
  safe, if not the requested variant.
