# Batched review of oversized diffs

AMF's AI reviews send the whole diff to a headless agent in one prompt.
When that diff is larger than the review model's context window the call
used to fail outright ("prompt is too long"), or — for the final-review
co-reviewer — the diff was silently truncated to a fixed byte budget.

Batched review replaces both failure modes: an oversized diff is split
into bounded pieces, each piece is reviewed on its own, and the results
are recombined. Coverage stays complete — nothing is dropped without
saying so.

## Where it applies

| Entry point | Trigger | Output |
| --- | --- | --- |
| `W` — AI PR review | The rendered `pr_review.ai_review` prompt is estimated to exceed the size budget | One combined review (summary + `path\|side\|line` findings), parsed and shown in the AI Review pane exactly as an un-batched run |
| Final-review AI co-review (`Ctrl+Space`, `f`, then the co-review key) | The current file's annotated body exceeds `CO_REVIEW_MAX_BODY` (~8000 bytes), or the rendered `review.co_review` prompt exceeds the size budget | The per-hunk-group `<line>\|<comment>` outputs, concatenated and parsed into draft line comments as usual |

Everything else — the plain-language walkthrough, the changeset overview,
PR-triage fix prompts — is per-file or already bounded and is not
batched. The plan interview has its own, non-diff guard (see below).

## How the split works

1. **Parse.** The unified diff is split into per-file sections, each a
   valid diff on its own, byte-for-byte reassemblable.
2. **Pack.** File sections are greedily packed, in order, into batches
   that stay under the estimated token budget. A file whose own diff
   exceeds the budget is set aside for hunk-splitting.
3. **Review each batch.** Every batch is sent through the harness-neutral
   runner (Claude, Codex, OpenCode, or Pi). If the harness *still*
   rejects a batch as too long, the batch is halved and each half
   retried, recursively, down to a single file.
4. **Hunk-split an oversized file.** A lone file that overflows is divided
   into groups of consecutive hunks — the file header is repeated in
   front of each group so a slice keeps file context. A single hunk that
   overflows even alone cannot be divided further.
5. **Recombine.** For `W`, every batch's findings are concatenated and
   run through a synthesis pass (`review.synthesis`) that merges
   duplicates and produces one summary + findings block. If the
   synthesis prompt itself overflows, each batch's findings are shrunk
   with `review.findings_summary` and synthesis is retried; if it still
   overflows the synthesis input is halved. If synthesis cannot run at
   all, the findings are concatenated verbatim under a stub summary.
   For co-review, the per-slice `<line>|<comment>` lines are simply
   concatenated — no synthesis pass.

Per-slice review loses cross-file context (a symbol that moved between
files, a type defined in another slice). The synthesis pass reconstructs
some of it; a hunk-level slice can still produce a false positive that
depends on an import or type it cannot see. The batch and hunk prompts
tell the model it is only seeing part of the change and to say so rather
than assert a bug when a concern depends on unseen code.

## Partial-coverage indicator

Coverage is never reduced silently.

- A hunk that exceeds the budget even as the only thing in its prompt, or
  a slice whose run failed, is recorded as **not reviewed** with the
  reason.
- For `W`, the review summary — in the AI Review pane, the post dialog,
  and any review posted to GitHub — is prefixed with a blockquote:

  ```
  > ⚠ Partial coverage — this diff was too large to review in one pass,
  > so it was split into slices.
  > N slice(s) could not be reviewed even after splitting:
  >   • `src/huge.rs` hunk 7 — exceeds the size budget even as a single hunk
  ```

  A line is added when the synthesis pass could not run and the findings
  were combined verbatim.
- For co-review, the status line appends `N hunk group(s) could not be
  reviewed` when any slice failed.

## Configuration

The size threshold is a **soft gate**: an estimate (bytes ÷ 4 ≈ tokens)
against a conservative per-harness budget. The hard backstop is the
adaptive halving that runs after a real "prompt too long" error, so the
threshold being a little wrong only changes how many batches there are,
never whether coverage is complete.

Defaults (`headless::default_prompt_budget_tokens`):

| Harness | Default budget (prompt tokens) |
| --- | --- |
| Claude | 128,000 |
| Codex | 128,000 |
| OpenCode | 96,000 |
| Pi | 96,000 |

Override:

- **Globally** — `review_prompt_budget_tokens` in
  `~/.config/amf/config.json` (a number of tokens).
- **Per repository** — `review_prompt_budget_tokens` in the repo's
  `amf.json`, which overrides the global value.
- `0` disables the pre-send split entirely. For the `W` review AMF then
  only reacts to an actual "prompt too long" error, retrying that run as a
  batched review with adaptive halving. Co-review has no post-failure
  retry, so with `0` an oversized file is sent in one pass with its body
  bounded by a visible "diff truncated" marker.

## Editable prompts

The batched-review prompts are in the headless-prompt registry and are
editable through the prompt-overrides overlay (dashboard `E`, or leader
`E` in a session) at feature / project / global scope, like every other
headless prompt:

| Id | Role |
| --- | --- |
| `review.batch` | Review one bounded slice of the diff |
| `review.hunk_split` | Review one hunk group of a file too large to review whole |
| `review.synthesis` | Combine every slice's findings into one review |
| `review.findings_summary` | Shrink one slice's findings when the synthesis prompt overflows |

## Very large refactors: review commit by commit

Batched review keeps a huge diff reviewable, but per-slice review is a
quality trade-off against a hard failure — it cannot reason across files.
For a large refactor (a rename sweep, a module move, a mechanical API
change) the better workaround is to keep the unit of review small in the
first place:

- Land the change as a series of focused commits and open a PR per
  commit, or
- `git rebase -i` an existing branch to split one megacommit into
  reviewable steps, then run `W` (or a commit-scoped review) on each.

Each commit then fits in one prompt with its full context intact.

## Plan interview (non-diff)

The plan-interview prompts (adaptive round, synthesis, advisory review)
are not diffs and are not batched. They have their own guard: if the
assembled prompt is estimated to overflow, AMF drops the repository
`README` / `CLAUDE.md` excerpts from that one prompt and retries, noting
the trim in the interview dialog's footer. If it still overflows, the
prompt is sent as is with a note that it may not fit. The same
`review_prompt_budget_tokens` setting controls this threshold; `0`
disables the guard.
