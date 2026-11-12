# Batched review of oversized diffs

Implementation notes for agents and contributors. Source is authoritative
when it disagrees; update this file in the same change.

When an AI review's diff would overflow the model, it is split into bounded
prompts and the findings recombined. Full rationale in
`docs/batched-review.md`; `AMF_PLAN.md` has the design decisions.

- **`src/diff_split.rs`** — lossless text-level splitting. `SplitDiff::parse`
  (`split_inclusive('\n')`, CRLF- and no-trailing-newline-safe) →
  `FileSection { path, header, hunks: Vec<HunkGroup> }`. `pack_file_sections`
  greedily packs sections into `ReviewBatch::{Files, OversizedFile}` under a
  token budget; `split_file_by_hunk` divides an oversized file into
  `HunkSubunit`s (header repeated per slice, `oversized` flag for an
  un-splittable lone hunk). `merge_hunk_findings` + `HunkOutcome` build the
  deterministic per-file block.
- **`src/review_batch.rs`** — orchestration. `trait BatchReviewRunner`
  (`HeadlessBatchRunner` = prod; `review` renders `review.batch` per file
  slice, `review_hunk` renders `review.hunk_split` per hunk group with
  `{{file_path}}` / `{{hunk_label}}` populated) and `trait SynthesisRunner`
  (`HeadlessSynthesisRunner`). `review_batches` runs each batch, halving on
  `PromptTooLong` down to a hunk, recording an un-reviewable slice as
  `UncoveredSlice` rather than dropping it. `synthesize` folds the per-batch
  texts through `review.synthesis` (shrinking via `review.findings_summary`
  and halving on overflow; deterministic concat fallback with
  `synthesis_ran=false`). `batched_review` chains parse → pack → review →
  synthesize into one `SynthesizedReview { text, synthesis_ran, uncovered }`.
  `BatchProgress` enum feeds the UI.
- **Size estimation / typed error** live in `src/headless.rs`:
  `estimate_prompt_tokens` (bytes ÷ `PROMPT_ESTIMATE_BYTES_PER_TOKEN`),
  `default_prompt_budget_tokens` (Claude/Codex 128k, OpenCode/Pi 96k),
  `will_overflow` / `will_overflow_with_budget` (budget `0` = gate off), and
  `PromptTooLong` + `is_prompt_too_long_message` / `as_prompt_too_long`
  (best-effort classifier over the four CLIs' overflow phrasings), emitted by
  `run_command` / `run_jsonl_command`.
- **Wired into** the `W` AI PR review (`app/ai_review.rs`: `begin_ai_pr_review`
  resolves `review.batch`/`.hunk_split`/`.synthesis`/`.findings_summary` +
  `App::review_prompt_budget`; `run_ai_pr_review` branches to
  `run_batched_ai_pr_review` when the pre-send estimate overflows **and** when
  the single pass itself returns `PromptTooLong` — so a `0` budget still gets
  the batched fallback with adaptive halving; `batched_coverage_note` prepends
  the "⚠ Partial coverage" banner to `AiReviewOutcome::summary`) and
  final-review co-review (`app/review.rs`: `generate_co_review` → worker thread
  `run_batched_co_review` → `DiffViewerState::co_review_bg` → `poll_co_review`
  → `apply_co_review_text`; a `0` budget skips the pre-send hunk-split entirely
  and the single pass truncates the body with a visible marker).
  `review_destination.rs` is untouched. The plan
  interview has a non-diff guard instead: `plan_interview::guard_context_for_prompt`
  drops the README/`CLAUDE.md` excerpts and notes it in the dialog footer.
- **Config**: `AppConfig::review_prompt_budget_tokens` (global) /
  `ExtensionConfig::review_prompt_budget_tokens` (project `amf.json`,
  project-over-global), resolved by `App::review_prompt_budget(repo, harness)`.
