# Headless runs, prompts and reasoning

Implementation notes for agents and contributors. Source is authoritative
when it disagrees; update this file in the same change.

## Runner and registry

**HeadlessRunner** (headless.rs):

- Harness-neutral one-shot runs for Claude, Codex, OpenCode, and Pi
- Restricted no-tools mode for context-complete prompts
- Read-only repository tools for directed plan revisions, and for the
  round/synthesis/critique passes when the interview has attached reference
  docs (see "Plan-interview reference docs" below)
- Harness selection and fallback for plan interviews

**Prompt registry** (`src/prompts/`): the single home for every headless
prompt AMF sends (see "Editable Headless Prompts" below). `mod.rs` holds
`PromptId` (24 stable ids), `PromptSpec` (title/summary/placeholders/
`default_template`/`harness_variants`), and `resolve_template_layered` /
`resolve_prompt_layered`. `defaults.rs` is the built-in template text moved
out of the call sites. `resolve.rs` has `PromptContext` +
`render_template` (unvalidated `{{token}}` substitution — missing/unknown
tokens render literally). `project.rs` reads project-scope overrides from
`amf.json`. Call sites resolve via `App::resolve_headless_prompt` /
`resolve_headless_template` (`app/mod.rs`), which assemble the feature (DB) →
project (`amf.json`) → global (DB) → built-in layers.

## Editable Headless Prompts

Every one-shot ("headless") AI call AMF makes runs a template from a central
registry that the user can view and override. See
`docs/backlog/editable-prompts-call-site-inventory.md` for the call-site map
and `AMF_PLAN.md` for the design decisions.

- **Registry (`src/prompts/`).** `PromptId::ALL` is the 24 stable ids
  (`plan_interview.round`/`.synthesis`/`.critique`/`.directed_revision`/
  `.investigation`/`.investigation_merge`/`.quick_round`/`.quick_synthesis`,
  `learning.answer`,
  `review.walkthrough`/`.co_review`/`.changeset_overview`/`.diff_explain`,
  `review.question`/`.question_draft` (review questions,
  `app/review_questions/`),
  `pr_review.ai_review`, `review_memory.bootstrap`/`.compact`/`.ai_summary`,
  `session.summary`, and the batched-review set
  `review.batch`/`.hunk_split`/`.synthesis`/`.findings_summary`). `defaults.rs`
  holds the built-in text. The 8
  plan-interview templates keep a single `{{interview_input}}` token carrying
  the exact JSON payload the models see today (the drift-guard test
  `plan_interview_defaults_stay_in_sync_with_the_tuned_prose` pins the six
  full-mode ones to the `plan_interview::*_PROMPT` prose, which is duplicated because a `const`
  can't be `concat!`-ed); the other 16 use granular tokens.
- **Interpolation is unvalidated.** `render_template` substitutes `{{name}}`
  from a `PromptContext`; a token with no value — declared or not — is left
  literally, and substituted values are never re-scanned. An override may drop
  or add tokens freely.
- **Three override scopes.** Feature (keyed by workdir path) and global
  overrides live in `amf.db`'s `prompt_overrides` table (`MIGRATION_034`,
  `src/db/prompt_overrides.rs`: `OverrideScope::{Feature,Global}`, CRUD +
  `PromptOverrides` in-memory view with a no-DB fallback). Project overrides
  live in `amf.json` under `ExtensionConfig::prompt_overrides`
  (`HashMap<prompt_id, PromptOverrideEntry{ template?, harnesses }>`,
  `src/prompts/project.rs`) — **not** `.amf/prompts/`, because `.amf/` is
  gitignored dir-wide, and **not** merged from the global `extension` block.
  Harnesses are keyed by `AgentKind::slug()` (`claude`/`codex`/`opencode`/
  `pi`).
- **Precedence.** `resolve_template_layered` (`src/prompts/resolve.rs`):
  feature → project → global → built-in, nearest wins; within the winning
  layer a per-harness template beats the shared one, so a nearer *shared*
  override beats a farther *per-harness* one. Once any layer supplies an
  override the built-in default is never read (silent "default drift"). Call
  sites go through `App::resolve_headless_prompt` / `resolve_headless_template`
  (`app/mod.rs`), read fresh each call.
- **Manager overlay.** `AppMode::PromptOverrides(Box<PromptOverridesState>)`,
  `app/prompt_overrides.rs` / `handlers/prompt_overrides.rs` /
  `ui/dialogs/prompt_overrides.rs`. Dashboard `E` / leader `E`. List → edit
  the effective template → `Ctrl+S` → scope picker (Feature only with a
  feature + DB, Project only with a repo, always Global) → harness picker
  (shared / one) → save. `d`,`d` clears the effective override (every
  per-harness row at that scope).
- **Pre-call notice.** `AppMode::PromptPrecall(Box<PendingPrecall>)`,
  `app/precall.rs`. `precall_gate(action, harness, rendered)` is called by
  each **user-initiated** gated call site right before it spawns; it stashes
  the originating mode and returns `false` (caller returns without spawning).
  `precall_confirm` restores the mode, sets `precall_cleared`, and
  re-invokes the same `start_*` via `dispatch_precall` — the re-run
  re-resolves the prompt so an override saved from `e` applies. `precall_edit`
  opens the manager focused on the prompt (`App::precall_return` brings the
  notice back on close). `precall_cleared` is wiped after every
  `dispatch_precall` so a fall-through (e.g. a round with no harness →
  synthesis) can't leave a stale clearance. **Automated** runs
  (`learning.answer`, `session.summary`) call `announce_headless_run` — a
  toast, never the modal — so a queued batch can't deadlock.

## Reasoning level (headless runs)

Wherever AMF lets you pick a headless model, it also lets you pick how hard
the model thinks.

- **One vocabulary, per-harness flags.** `headless::ReasoningLevel`
  (`minimal`..`max`) maps through `reasoning_args`: Claude `--effort`, Codex
  `-c model_reasoning_effort="…"`, OpenCode `--variant`. Pi has no verified flag,
  so it offers none. `ReasoningLevel::supported_for(harness)` is the verified
  set; a level the harness cannot express is **dropped, never passed through**
  (Codex has no `max`; Claude has no `minimal`).
- **`ModelSel`** (`model` + `reasoning`) is what every `HeadlessRunner` entry and
  `ClaudeLauncher::spawn_headless` accept via `impl Into<ModelSel>`, so a
  caller with only a model (`Option<&str>`) is unchanged.
- **Pickers.** The AI Review (`A`) and Expert plan review model pickers carry
  `AiModelPickState::{reasoning_levels, reasoning}`; `h`/`l` (or ←/→) cycle
  Default → each level on the same screen. The row is hidden for a harness
  with no levels. Expert reasoning is **not persisted** with the interview
  draft (it would need a migration on the shared `amf.db`); a resumed draft
  falls back to the `plan_preflight` config entry.
- **Config.** `review_reasoning` / `review_reasonings` mirror `review_model` /
  `review_models` (`AppConfig::review_reasoning_for`; Expert plan review never
  inherits the shared default). Applies to the picker seed, Final Review
  (walkthrough, co-review, overview, diff explain) and review memory.
- **Attribution** records the level (`AiReviewAttribution::reasoning`) only when
  the harness could use it.
