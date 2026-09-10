# Making premium models affordable through AMF

- **Status:** Backlog
- **Owner:** unassigned
- **Scope:** Options and quantitative planning reference; individual feature
  designs and implementation plans are still to be written.
- **Evidence date:** 2026-09-09
- **Relates to:** [Token-efficient agent sessions](token-efficiency-plan.md),
  [per-session agent usage](per-session-usage-plan.md),
  [context telemetry](../context-usage-telemetry.md),
  [repository guidance](../../AGENTS.md)

## Purpose

Make expensive models such as GPT-6 Astra and Claude Fable practical for
everyday development through AMF. Control which work reaches them, what
context accompanies it, and when previous results can be reused. Preserve
their value for difficult reasoning while moving routine execution,
evidence gathering, and bookkeeping into AMF or less expensive models.

This document captures eight options, their mechanisms, worked savings
examples, implementation boundaries, and questions for later feature plans.
Complexity is not a constraint on the options considered here. The existing
token-efficiency plan remains a related implementation backlog; this document
extends the design space and does not replace it or mark its work complete.

All savings examples are hypothetical calculations, not AMF benchmarks or
promises. No workflow comparison or paid-model experiment was run to produce
them. Provider behavior and prices were checked against official documentation
during the original investigation. Recheck them when planning implementation.

## What to optimize and how to count it

Measure these separately:

1. **Total tokens:** normalized input plus generated output across every model,
   including consultations, retries, summaries, and background work.
2. **Premium-model tokens:** the portion consumed by Astra/Fable or another
   configured premium model. Routing can reduce this while increasing total
   tokens.
3. **Cost per accepted change:** the complete workflow cost at an equivalent
   quality bar, including failed attempts and verification.
4. **Context occupancy:** the current request's context size. Cumulative input
   usage can exceed a model's window many times because context is reused
   across requests.

Cache hits reduce input cost but do not remove logical input tokens or free
context-window space. Reducing context does not imply an equal percentage
reduction in cost. Reasoning is part of billed generated output on OpenAI
reasoning models, even when the visible response is short. See the
[reasoning documentation](https://developers.openai.com/api/docs/guides/reasoning).

Normalize provider fields before pricing: some report cached input as a
subset of input, while others report separate categories. Likewise, do not
add reasoning to output when it is already included. Preserve raw usage and
its provenance so normalization can be audited.

For disjoint normalized categories, use:

```text
cost = (uncached_input * uncached_rate
      + cache_write_input * write_rate
      + cache_read_input * read_rate
      + generated_output_including_reasoning * output_rate) / 1,000,000
      + applicable tool or service fees
```

Apply model-specific thresholds and service-tier modifiers before summing
requests. A single rate applied to a mixed-model session is insufficient.

### Pricing snapshot used in the examples

Standard API USD per million tokens, as checked on the evidence date:

| Model | Uncached input | Cache write | Cached input | Output |
| --- | ---: | ---: | ---: | ---: |
| GPT-6 Astra | $10 | $12.50 | $1 | $50 |
| Claude Fable 5 | $10 | $12.50 | $1 | $50 |
| Claude Fable 5.1 | $10 | $12.50 | $0.25 | $50 |
| Claude Sonnet 5 | $2 | $2.50 | $0.20 | $10 |

Claude write prices here use the five-minute cache. Different retention,
service tiers, regional pricing, or subscription allowances require separate
accounting. These are not a claim about a user's subscription bill or quota.
Sources: [Astra pricing](https://developers.openai.com/api/docs/models/gpt-6-astra)
and [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing).

At these rates, avoiding 10,000 generated tokens saves $0.50 on the premium
models. Avoiding 10,000 cached input tokens saves $0.01 on Astra/Fable 5 or
$0.0025 on Fable 5.1. This makes unnecessary reasoning and repeated attempts
especially valuable targets.

## Existing AMF foundations and gaps

The code snapshot was inspected directly on `chore/codebase-maintainability`.
Confirm these observations against the target branch when writing a feature
plan. Older backlog descriptions should not be treated as a complete account
of current behavior.

| Foundation | Current evidence | Opportunity |
| --- | --- | --- |
| Headless execution | [`HeadlessRunner`](../../src/headless.rs) supports harness selection, optional models, and progress/usage reporting. | Add explicit workflow profiles and bounded expert consultations. |
| Session and feature ownership | Sessions, worktrees, and workflow orchestration already belong to AMF. | Own task state, handoffs, validation artifacts, and model routing. |
| Usage accounting | [`token_tracking.rs`](../../src/token_tracking.rs) records usage, but `TokenPricingConfig` defaults to one Sonnet-based rate table. | Attribute actual models and price each usage segment correctly. |
| Context telemetry | [`context_tracking.rs`](../../src/context_tracking.rs), [`context_collectors.rs`](../../src/context_collectors.rs), and [`context_hints.rs`](../../src/app/context_hints.rs) already exist. | Build economic decisions on existing context/reset signals rather than introducing another occupancy counter. |
| Session summaries | [`summary.rs`](../../src/summary.rs) uses the selected harness, recent pane lines, and no explicit model. | Deterministic summaries, separate utility models, and artifact reuse. |
| Transcript export | [`transcript.rs`](../../src/transcript.rs) exports Claude user/assistant text and finds the latest transcript for a workdir. | Bind handoffs to the selected session and replace unbounded textual history with structured checkpoints. |
| AI review cache | [`ai_review_cache.rs`](../../src/db/ai_review_cache.rs) reuses findings for an unchanged PR head. | Invalidate and reuse review evidence across subsequent commits. |
| Review memory | [`review_memory.rs`](../../src/app/review_memory.rs) stores recurring findings. | Reuse validated procedures and selectively retrieve relevant decisions. |
| Local storage and parsing | [`Cargo.toml`](../../Cargo.toml) already includes SQLite and tree-sitter. | Build an incremental context index; the dependencies alone do not provide one. |
| Feature presets | [`FeaturePreset`](../../src/extension.rs) selects a harness and workflow settings, without model/effort fields. | Add stage-specific model and effort profiles. |

In particular, the older token-efficiency backlog says summaries always use
Claude and describes context visibility as missing. The current code has
progressed beyond those statements. Reconcile relevant status claims when
turning an option into a feature plan.

## Options at a glance

The savings column gives the denominator for each worked example. These
percentages overlap and must not be added together.

| ID | Option | Example saving | Principal dependency |
| --- | --- | --- | --- |
| OPT-01 | Expert Assist | 55% workflow cost; 47.5% if workers need 50% more work | Routing, explicit profiles, bounded handoffs |
| OPT-02 | Tools that return evidence | 95% of a large tool result's input tokens | Structured execution and artifact retrieval |
| OPT-03 | Shared repository context | 75% of exploration input; 30% of total input if exploration is 40% | Incremental index and invalidation |
| OPT-04 | Task checkpoints | 90% of inherited context in the example | Reliable session identity and context lifecycle |
| OPT-05 | Cache and threshold awareness | 84–91% of repeated-prefix input cost versus no caching | Cache telemetry and supported request controls |
| OPT-06 | Incremental review | 68% of review input across five review passes | Review evidence and dependency tracking |
| OPT-07 | Stage-specific reasoning | 51% of generated output in the example | Model/effort controls and quality evaluation |
| OPT-08 | Reusable procedures and artifacts | 82% of premium tokens for repeated tasks; 100% of a call on an exact artifact hit | Applicability checks, validation, cache keys |

## OPT-01: Expert Assist

Feature design: [Expert Assist design and feasibility](expert-assist-design.md).
The design investigation and opt-in prototype are complete, including the
consultation form, bounded owned jobs, durable request/evidence state, and
target-validated editable handoffs. Real provider conformance and cost/quality
trials remain pending; the worked savings below remain hypothetical.

### Behavior and implementation shape

Maintain separate implementer and expert sessions. An inexpensive model
handles routine implementation. Astra/Fable receives a bounded consultation:
the question, relevant source, observed failures, attempted fixes, and
acceptance criteria. It may request more evidence and returns a decision or
targeted patch. AMF runs validation and returns the result to the implementer.

For an AMF lifecycle bug, an expert request could be: "These two cleanup paths
disagree about process ownership; identify the correct invariant and necessary
changes." The expert need not inherit unrelated implementation chatter.

Use explicit escalation signals such as repeated identical failures,
contradictory requirements, or changes to important architectural contracts.
Allow direct expert work for tasks already known to be difficult. Repeatedly
trying a cheap model first is not always economical.

Reuse the headless runner, feature/session ownership, and worktree isolation.
Coordinate mutation ownership so implementer and expert do not race to edit
the same files. Keep bounded results and source references in shared task
state instead of copying entire conversations between agents.

### Savings calculation

Assume the expert retains 25% of the baseline billable workload, 75% moves
to a model at 20% of the effective rate, and coordination costs another 5%
of the original total:

```text
remaining cost = 0.25 + (0.75 * 0.20) + 0.05 = 0.45
saving = 55%
```

If the worker needs 50% more work:

```text
remaining cost = 0.25 + (0.75 * 1.50 * 0.20) + 0.05 = 0.525
saving = 47.5%
```

This simplified scenario assumes comparable token-category mixes and effective
rates. Actual model tokenization, caching, and retries require request-level
accounting. Total tokens may increase while premium usage and cost fall.

### Questions and evaluation for a feature plan

- Define the consultation schema, evidence budget, escalation policy, and
  ownership of edits and long-running commands.
- Choose when expert review is required and when deterministic checks suffice.
- Compare accepted-change cost against expert-only execution, including failed
  cheap attempts, expert rework, and coordination.
- Track handoff omissions and defects missed by the worker. Test passage alone
  is not proof that the two workflows have equal quality.

## OPT-02: Tools that return evidence instead of large logs

### Behavior and implementation shape

Provide AMF operations such as `run_checks`, `inspect_failure`, and
`read_symbol`. These are proposed tools, not existing commands. Retain full
output locally and return structured evidence: command identity, exit status,
failure locations, relevant diagnostics, and an artifact reference for more.

For this repository, a validation result could report formatting and Clippy
success, the failing test name, and its diagnostic. The full test log remains
available on demand. AMF can execute a known validation sequence without an
expensive model deciding and interpreting every routine step.

Use deterministic parsers first. Retain stderr, failures, and unexpected
formats; fall back to an explicit bounded excerpt with an omission marker.
An artifact reference must be usable through a retrieval tool. Writing a log
to disk without giving the agent access does not preserve usable evidence.

### Savings calculation

```text
raw result = 30,000 tokens
structured evidence = 1,500 tokens
reduction = 28,500 tokens = 95% of this tool result
```

If the result would appear in ten subsequent model requests, the reduction
is 285,000 cumulative input tokens. This assumes it remains in history for all
ten requests; native pruning or compaction reduces the additional benefit.
Price each avoided appearance according to whether it would be written,
uncached, or read from cache. Extra retrieval calls reduce the net saving.

### Questions and evaluation for a feature plan

- Which command families can be parsed reliably, and how are unknown formats
  exposed without hiding failure evidence?
- How are artifacts scoped, retained, and invalidated after new runs?
- Compare raw versus returned tokens, follow-up reads, model turns, and
  diagnostic accuracy. Include parser CPU/storage cost separately.
- Preserve required repository validation; reducing log volume must not mean
  skipping checks or hiding regressions.

## OPT-03: A repository context service shared across worktrees

### Behavior and implementation shape

Maintain an incremental index of symbols, references, module ownership,
associated tests, and relevant project decisions. Retrieve small source-backed
evidence packets for a task rather than rediscovering the repository in each
session.

A lifecycle task could receive the relevant `app/` orchestration, `TmuxOps`
contract, cleanup implementation, and associated tests. Excerpts carry file
locations and content hashes. Summaries help locate authoritative source and
are invalidated when their evidence changes.

Share entries for identical file content across worktrees and overlay each
worktree's modified, untracked, and deleted files. Include repository identity,
configuration, and relevant dependency versions in validity checks. Tree-sitter
provides syntax; semantic references and test relationships require additional
analysis, potentially through language-specific tooling.

### Savings calculation

```text
baseline exploration = 60,000 input tokens
replacement packet = 10,000 tokens
follow-up source reads = 5,000 tokens
exploration reduction = 45,000 / 60,000 = 75%
```

If exploration accounts for 40% of all task input, total input falls by
`0.40 * 0.75 = 30%`, before index construction and maintenance overhead.
This is not a 30% reduction in output or total cost. Any model-generated index
content must have its construction cost amortized across actual reuse.

### Questions and evaluation for a feature plan

- Start with supported languages and a deterministic retrieval baseline;
  decide whether semantic search or model summaries add measurable value.
- Specify branch overlays, unsaved-file handling, and dependency invalidation.
- Measure missing-relevant-source rate, follow-up reads, startup latency,
  indexing cost, and tokens per accepted change.
- Prevent stale summaries from overriding source evidence.

## OPT-04: Explicit task checkpoints and fresh continuations

### Behavior and implementation shape

At a completed phase, offer a fresh session with a bounded checkpoint:

- Goal, scope, constraints, and settled decisions.
- Current patch and relevant source references.
- Validation commands and outcomes, unresolved failures, and active work.
- Remaining tasks and the next concrete action.

Construct most fields from AMF state, Git, and execution records. Use a model
only for information that requires interpretation, such as a design rationale
not recorded elsewhere. Preserve the original session and make omitted history
retrievable. Bind checkpoints to the selected provider session, not merely the
most recently modified transcript in a workdir.

Native compaction and fresh handoffs are separate actions. Native compaction
can preserve provider-specific state; fresh handoffs support portable,
inspectable task state. Use documented harness capabilities and explicit user
control for transitions that discard active context. OpenAI's
[compaction guide](https://developers.openai.com/api/docs/guides/compaction)
describes its native mechanism; this proposal does not assume all harnesses
expose the same controls.

### Savings calculation

Replacing 120,000 tokens of inherited context with 12,000 removes 90% of that
context. Across ten subsequent requests, holding other work constant:

```text
avoided input = (120,000 - 12,000) * 10 = 1,080,000 tokens
```

If every avoided token would have been a cache read, the gross benefit is
$1.08 on Astra or $0.27 on Fable 5.1. Checkpoint generation, new prefix writes,
retrieval, and reorientation must be subtracted. The net saving may be negative.

Use an economic decision rule rather than context percentage alone:

```text
rotate when expected avoided future input cost
          + expected benefit from avoiding pricing thresholds or rework
          > checkpoint generation + cache rebuilding + expected reorientation
```

### Questions and evaluation for a feature plan

- Define checkpoint provenance, editing, resumption, and recovery from omitted
  decisions. Distinguish finished commands from still-running processes.
- Estimate the number of remaining calls and actual cache effectiveness.
- Compare continuation, native compaction, and fresh handoff on equivalent
  tasks, including lost decisions and repeated investigation.
- Reuse existing context/reset telemetry and preserve process lifetimes.

## OPT-05: Cache economics and pricing-threshold awareness

### Behavior and implementation shape

Keep reusable instructions, tool definitions, and context prefixes stable;
place changing task information after them. Observe cache writes, reads, and
misses. Identify prompt changes controlled by AMF that prevent reuse.

Request-level caching controls require an integration that actually owns or
exposes those settings. A prompt cache key alone does not make different
prefixes equivalent. Tool definitions and relevant request configuration can
affect prefix matching, and compaction may invalidate an earlier prefix.
See [OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching).

### Savings calculation: reusable prefix

For a 100,000-token prefix used in 20 requests, with all reuse occurring while
the cache is valid and only one initial write:

| Treatment of this prefix | Calculation | Cost |
| --- | --- | ---: |
| Uncached every time | `20 * 0.1 * $10` | $20.00 |
| Astra: one write and 19 reads | `0.1 * $12.50 + 19 * 0.1 * $1` | $3.15 |
| Fable 5.1: one five-minute write and 19 reads | `0.1 * $12.50 + 19 * 0.1 * $0.25` | $1.725 |

That is 84.25% and 91.375% lower repeated-prefix input cost, respectively,
with zero reduction in logical input tokens. New input, output, and tool fees
are outside this prefix-only example. The incremental benefit is small or
zero if the existing harness already achieves the same cache hits.

### Savings calculation: Astra's long-context threshold

The checked Astra pricing charges 2x input/cache rates and 1.5x output rates
for the full request above 272,000 input tokens. See
[Astra pricing](https://developers.openai.com/api/docs/models/gpt-6-astra).

Assume ordinary uncached input with no cache-write charges and 4,000 output
tokens in both requests:

```text
300,000 input: (0.300 * $20) + (0.004 * $75) = $6.30
250,000 input: (0.250 * $10) + (0.004 * $50) = $2.70
```

This is 57.1% lower request cost for 16.7% fewer input tokens, assuming omitted
context does not cause extra work. It is a model-specific threshold example,
not a universal argument for shrinking every request.

### Questions and evaluation for a feature plan

- Record price-source date, service tier, cache retention, model identity, and
  applicable thresholds. Display unknown rates explicitly.
- Distinguish AMF-caused misses from native harness/provider behavior.
- Compare against actual current cache performance, not an artificially
  uncached baseline. Include cache rebuilding after model/effort changes.
- Show threshold estimates with uncertainty when AMF cannot observe the
  complete rendered request. Do not remove required context just to fit a tier.

## OPT-06: Incremental review with reusable evidence

### Behavior and implementation shape

Extend the current unchanged-head review cache across subsequent commits.
Record which source regions, dependencies, instructions, and review scope
support each finding. Review changed regions and their affected callers and
invariants while retaining unaffected evidence.

Changes to shared contracts, dependency versions, base branches, review
instructions, or unresolved dependency relationships should trigger broader
review. Preserve the distinction between reusing an existing finding and
claiming that new code has been reviewed.

### Savings calculation

```text
five complete reviews = 5 * 80,000 = 400,000 input tokens
one complete review + four incremental reviews
                     = 80,000 + (4 * 12,000) = 128,000 input tokens
reduction            = 68% of review input
```

This assumes the changes really permit bounded incremental reviews. It does
not claim the same reduction in reasoning/output or account for evidence
construction. Cross-cutting changes can eliminate most reuse.

### Questions and evaluation for a feature plan

- Define review coverage and invalidation keys: repository, base/head state,
  scope, prompt/rules, model configuration, and referenced dependencies.
- Define when a final full review is still needed and include it in costs.
- Compare incremental findings with full-review findings on the same final
  patch. Track missed defects, stale findings, false positives, and total cost.
- Reuse PR identity and review progression owned by existing AMF workflows.

## OPT-07: Reasoning effort by task stage

### Behavior and implementation shape

Add model and effort settings to feature presets, sessions, and workflow
stages. Spend more reasoning on uncertain design and difficult debugging;
use less for well-specified edits, routine summaries, and formatting fixes.

Keep requested settings separate from observed model/settings because users
can change them inside a harness. Probe supported capabilities and preserve
unknown values rather than silently mapping them to unrelated controls.

### Savings calculation

Assume an evaluated workflow maintains quality while reducing reasoning from
30,000 to 12,000 tokens and retaining 5,000 visible output tokens:

```text
baseline generated output = 30,000 + 5,000 = 35,000
optimized output          = 12,000 + 5,000 = 17,000
reduction                 = 18,000 / 35,000 = 51.4%
saved output cost         = 0.018 * $50 = $0.90
```

The reasoning reduction is an experiment assumption, not a guaranteed effect
of selecting a particular effort label. Another implementation attempt or a
cache miss can offset the saving. A small output cap can also produce an
incomplete response after paid reasoning; it is not a substitute for effort
evaluation. See [reasoning controls](https://developers.openai.com/api/docs/guides/reasoning).

### Questions and evaluation for a feature plan

- Define profiles for implementation, consultation, review, and utility work.
- Evaluate each supported model/effort combination on representative tasks.
- Track reasoning, visible output, incomplete responses, retries, cache
  effects, and accepted-change cost.
- Permit escalation without losing essential task state.

## OPT-08: Reusable procedures and cached artifacts

### Behavior and implementation shape

Convert recurring successful expert work into validated procedures with
applicability conditions, source/version assumptions, steps, validation, and
escalation conditions. Scripts or inexpensive models execute known cases;
the expert handles new exceptions.

Examples include adding a configuration field across persistence and UI,
updating a harness adapter, or diagnosing a recurring compiler failure. Store
compact procedures and retrieve only those relevant to the current task.
Appending every lesson to always-loaded instructions would create new waste.

Apply the same principle to utilities. Use deterministic session titles when
adequate, separately configured inexpensive models when semantics are needed,
and persisted cached artifacts for unchanged summaries or explanations.

### Savings calculation

```text
baseline repeated expert work = 20 * 15,000 = 300,000 premium tokens
recipe construction           = 15,000 premium tokens
exception/verification calls  = 20 * 2,000 = 40,000 premium tokens
new premium total             = 55,000 tokens
reduction                     = 81.7% of premium tokens
```

Worker execution, recipe maintenance, and failures are additional costs. The
assumption that every task needs only 2,000 premium verification tokens must
be evaluated against real recurring tasks.

An exact artifact-cache hit avoids 100% of the otherwise necessary model call.
That is a per-call saving, not a 100% workflow saving. Local lookup, validity
checks, and storage still have operational costs.

### Questions and evaluation for a feature plan

- Define recipe acceptance, applicability, expiration, and invalidation.
- Key artifacts by relevant source, instructions, model configuration, and
  external-data versions; include repository and workflow scope.
- Cache results and evidence, not the assumption that a previous mutation is
  safe to replay. Recheck preconditions before executing a procedure.
- Measure actual reuse, construction amortization, exceptions, and regressions.

## Combined worked scenario

Consider one accepted feature implemented entirely with Astra, compared with
an optimized workflow using Astra consultations and Sonnet 5 workers. This is
a separate hypothetical scenario; it is not derived by adding the eight
options' percentages.

Assume standard rates from the snapshot, requests below Astra's long-context
threshold, every fresh input token charged as a cache write, and generated
output including reasoning. Worker usage includes coordination and retries.
No additional hosted-tool charges are assumed.

| Work | Cache-write input | Cache-read input | Generated output | Total tokens | Cost |
| --- | ---: | ---: | ---: | ---: | ---: |
| Baseline: Astra does everything | 200,000 | 800,000 | 40,000 | 1,040,000 | $5.30 |
| Optimized: Astra expert work | 60,000 | 250,000 | 12,000 | 322,000 | $1.60 |
| Optimized: Sonnet 5 worker work | 100,000 | 400,000 | 20,000 | 520,000 | $0.53 |
| Optimized total | 160,000 | 650,000 | 32,000 | 842,000 | $2.13 |

```text
baseline cost = (0.200 * $12.50) + (0.800 * $1) + (0.040 * $50) = $5.30
expert cost  = (0.060 * $12.50) + (0.250 * $1) + (0.012 * $50) = $1.60
worker cost  = (0.100 * $2.50) + (0.400 * $0.20) + (0.020 * $10) = $0.53
```

The resulting reductions are 59.8% in cost, 69.0% in Astra tokens, and 19.0%
in total tokens. These are useful design objectives only if both workflows
meet the same acceptance and quality criteria. Real evaluation must account
for any indexing, checkpoint, recipe, or evaluation overhead not present in
these hypothetical counts.

## Architectural boundaries

AMF can already configure launches, prepare prompts, observe usage, manage
sessions/worktrees, and orchestrate headless jobs. This supports early work on
expert consultations, utility profiles, review reuse, and deliberate handoffs.

Observing a tmux pane does not expose or control every model request. Reliable
tool-result shaping, exact pre-request budgets, and cache breakpoints need
supported harness integrations or an optional AMF-controlled API execution
path. A prompt request to keep output short is not an enforced token budget.

Prefer native harness controls where they meet the requirement. For each
feature plan, document which capabilities are observed, advisory, configured,
or enforced, and which harness versions support them. An API execution path
would introduce substantial ownership of authentication, tools, streaming,
continuation, cancellation, accounting, and compatibility; it is an explicit
architectural option, not a hidden prerequisite for every feature.

Keep workflow orchestration in `app/`, persistence in `db/`, and rendering in
`ui/`. Preserve worker identity checks and process ownership. Inject tmux and
worktree operations through existing boundaries. Reuse incremental telemetry
and background scheduling so cost optimization does not regress startup or
normal refresh performance. A budget threshold must not terminate a tool
mid-edit merely to reduce spending.

## Shared measurement foundation

Before claiming savings, record per-request or best-available usage segments:

- Feature, AMF session, provider conversation, workflow stage, and parent job.
- Requested and observed provider/model, effort, service tier, and price date.
- Disjoint input categories, generated output, reasoning detail when reported,
  current context, and confidence/provenance for each value.
- Tool calls, artifact reads/hits, checkpoint events, retries, escalation,
  compaction, and cache rebuilding.
- Acceptance outcome, validation results, reviewer findings, and elapsed time.

Attribute child work once and include abandoned or failed paid calls. Unknown
usage must remain unknown rather than appearing as zero. Keep telemetry local
and avoid full transcript rescans in the normal refresh path.

Evaluate representative AMF tasks: a small UI fix, a configuration/persistence
change, a lifecycle bug, a refactor across modules, and a multi-commit review.
Use the same repository snapshots, task requirements, and acceptance checks.
Repeat enough tasks/runs to expose variance; report sample size and failure
rates. Pair automated checks with review of correctness and maintainability.

Compare one intervention at a time before evaluating combined workflows.
Report median and tail cost, total/premium tokens, retries, quality outcomes,
and elapsed time. Include preparation and background costs and distinguish
one-time infrastructure work from recurring inference. A cheap failed task
must not appear as a successful saving.

## Suggested planning sequence

1. **Correct accounting and establish the baseline.** Model attribution,
   pricing categories, child-job accounting, and outcome measurement support
   every option. This work may reveal which cost dominates actual usage.
2. **Prototype Expert Assist and evidence-returning tools.** OPT-01 and OPT-02
   directly target expensive reasoning, repeated exploration, and routine
   model turns while reusing existing headless workflows.
3. **Add explicit utility and reasoning profiles.** Parts of OPT-07 and OPT-08
   can ship independently with modest orchestration changes.
4. **Develop shared context, checkpoints, and incremental reviews.** OPT-03,
   OPT-04, and OPT-06 require careful identity and invalidation designs.
5. **Deepen request-level control where measured savings justify it.** OPT-05
   and enforced tool/request budgets may warrant structured harness adapters
   or an optional API runner after their incremental benefit is established.
6. **Build the reusable-procedure feedback loop.** Extend OPT-08 once real
   recurring tasks provide evidence of applicability and reuse.

The order is a recommendation, not a committed delivery schedule. Prefer the
next feature that addresses the largest measured cost without degrading
accepted-change quality.

## Turning an option into a feature plan

Use its stable OPT identifier and link back to this document. Each plan should
specify the user workflow, inspected current behavior, scope, owned modules,
data formats, harness capability requirements, and migration implications.
Include a concrete baseline, a falsifiable savings hypothesis with a defined
denominator, failure/recovery behavior, meaningful validation, and rollout
criteria. Carry forward uncertainties rather than converting examples into
promised performance targets.

## Progress

- [x] Capture the eight options and their worked savings examples.
- [x] Record existing AMF foundations, architectural boundaries, and sources.
- [x] Define measurement requirements and a suggested planning sequence.
- [ ] Establish measured cost and quality baselines on representative tasks.
- [ ] Create individual feature plans for selected options.
- [ ] Implement and evaluate selected features before reporting actual savings.
