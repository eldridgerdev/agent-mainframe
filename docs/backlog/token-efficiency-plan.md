# Token-efficient agent sessions

- **Status:** Backlog
- **Owner:** unassigned
- **Relates to:** [per-session agent usage](per-session-usage-plan.md),
  [plan-mode interview](plan-mode-interview-plan.md),
  [prompt library](prompt-library-plan.md),
  [final-review enhancements](final-review-enhancements-plan.md),
  [startup performance](startup-performance-plan.md), token tracking
  (`src/token_tracking.rs`), session launch (`src/tmux.rs`,
  `src/app/feature_ops.rs`), feature presets (`src/extension.rs`),
  session summaries (`src/summary.rs`), transcript handoff
  (`src/transcript.rs`), and injected agent instructions
  (`src/app/setup.rs`)

## Why / problem

AMF can show cumulative token usage and cost for Claude, Codex, and
opencode sessions, including a feature-level aggregate. That answers
"what has this session spent?", but it does not yet help the user make
the decisions that prevent avoidable usage:

- Features and presets cannot select a model, reasoning effort, or
  provider variant. Every interactive session starts with whatever the
  harness currently considers its default.
- The visible usage is primarily a lifetime total. AMF does not show
  the latest turn, recent burn rate, cache effectiveness, or estimated
  current-context pressure, so a long-running session can become
  expensive without an early signal.
- Session continuation is provider-dependent rather than a deliberate
  AMF policy. Claude normally resumes its stored session, while normal
  Codex and opencode restarts do not consistently resume their stored
  provider session. Neither path offers a clear "resume, compact, or
  start fresh with a handoff" choice.
- Forking with context defaults on and exports the entire textual
  Claude transcript selected as "latest for this workdir". It is not
  bound to the selected AMF session, has no size limit, does not work
  for other harnesses, and is written to `.claude/context.md` without
  an explicit handoff to the newly launched agent.
- Review Mode instructs the agent to append a prose note before every
  Edit or Write. This spends model output and tool calls throughout the
  implementation even though AMF can derive changed files from the diff
  and can already generate missing walkthroughs on demand.
- Several small AMF helper jobs use a paid model. Session summaries
  always invoke headless Claude even for another harness. Review
  walkthroughs and changeset overviews are cached only in the open
  viewer, so unchanged work can be regenerated after reopening.
- Cost calculation uses one global pricing table whose defaults are
  Claude Sonnet prices. A mixed-model or mixed-provider workspace can
  therefore show a plausible-looking but incorrect dollar total.
- AMF injects persistent instructions and six AMF skills into each
  supported worktree. Even small always-visible descriptions and
  instructions compound when they are included in every turn.

The goal of this plan is to move AMF from **usage accounting** to
**usage management** while keeping control with the user. AMF should
make the efficient path obvious, provide bounded one-key transitions
when context gets large, and remove token-consuming work that can be
done locally.

## Design principles

- **Measure before enforcing.** Usage warnings must be based on
  provider-specific data and clearly labeled as exact or estimated.
- **Use native harness controls.** AMF should configure and invoke each
  harness's supported model, effort, compaction, and output-limit
  controls rather than inventing a second context implementation.
- **Never destroy context silently.** Compaction and fresh-session
  rotation require a user action. A budget threshold must not kill an
  agent while it is editing or running a tool.
- **Prefer zero-token local work.** Derive summaries, changed-file
  lists, prompt warnings, and cache keys locally when semantic model
  output is not required.
- **Bound every generated handoff and utility prompt.** Raw transcripts,
  diffs, logs, and repository context must have explicit input and
  output budgets.
- **Do not trade tokens for retries blindly.** Economy settings are a
  selectable profile, not a universal default. Users can escalate a
  difficult task without rebuilding the session.
- **Keep telemetry local.** Usage history, budgets, and recommendations
  stay in AMF's local SQLite store unless the user explicitly exports
  them.
- **Do not regress startup.** Reuse incremental provider parsing from
  the usage and startup-performance work; do not add new recursive
  transcript walks per refresh.

## Proposed design

### 1. Session efficiency profiles

Add named efficiency profiles that can be selected in feature creation,
feature presets, and when adding an agent session. Start with three
built-ins, all overridable in global or project `config.json`:

- **Economy:** lower-cost model/variant and low reasoning effort;
  intended for routine edits, summaries, and well-specified fixes.
- **Balanced:** provider default model with medium/default effort;
  suitable as the default migration behavior.
- **Deep:** stronger model and high reasoning effort; intended for
  architecture, difficult debugging, and final review.

Store the selected profile on `Feature` as the default for new sessions
and store the requested launch settings on each `FeatureSession`.
Presets gain an optional profile name. A session may override its
feature default without mutating sibling sessions.

Use a small harness adapter to translate generic intent into supported
arguments:

```text
Claude    model -> --model       effort -> --effort
Codex     model -> --model       effort -> model_reasoning_effort
opencode  model -> --model       effort -> --variant / provider options
Pi        no assumptions until its installed CLI exposes stable controls
```

Keep **requested configuration** separate from **observed model**.
Users can change models inside a harness, so token readers should record
the model reported by each provider event or transcript whenever
available. The UI shows a mismatch when the observed model no longer
matches the AMF profile.

Add a separate utility-inference profile. A one-line session summary
should not silently inherit the expensive model selected for a difficult
coding task, and an AI code review should not silently inherit an
economy summarization model.

### 2. Provider- and model-aware usage snapshots

Extend the current cumulative `SessionTokenUsage` path with a transient
snapshot suitable for decisions:

```text
SessionUsageSnapshot
  cumulative usage
  latest-turn usage
  recent deltas / burn rate
  observed provider + model
  estimated active-context tokens
  known context-window size, when available
  cache-read and cache-write ratios
  source confidence and sampled_at
```

Provider readers should expose the latest request/turn instead of only
summing the transcript. For Claude, keep parent and subagent usage as
separate fields before aggregating so the UI can identify fan-out. For
Codex and opencode, preserve reasoning-token data and provider-reported
last-turn totals where available.

Replace the single pricing object with a provider/model price catalog:

- Key built-in prices by normalized provider and model identifier.
- Let global and project config override or add entries.
- Segment cost when the observed model changes mid-session.
- Show "price unknown" instead of applying an unrelated default.
- Keep token counts usable even when price is unknown.
- Add Pi only after a stable local usage source is identified.

Complete the observability items in the per-session usage plan as part
of this work: log source binding, provider session ID, confidence,
model changes, and ambiguous inference. Exact provider-session binding
remains the foundation; inferred data must not drive hard budget
behavior.

### 3. Context-pressure UI and budgets

Add a compact context indicator to session rows and the selected-session
sidebar. Prefer a percentage when AMF knows the observed model's context
window; otherwise show the estimated active token count and an
"estimated" marker. The expanded view should include:

- Latest-turn input, output, reasoning, and cache tokens.
- Recent burn rate over a small turn/time window.
- Estimated active context and known context limit.
- Cumulative session and feature usage/cost.
- Parent versus subagent contribution where supported.
- Selected efficiency profile and observed model.

Add configurable soft thresholds, initially at the session and feature
levels:

- Context warning thresholds, such as 60%, 80%, and 95%.
- Cumulative token or dollar warning thresholds.
- Per-utility-call maximum input and output budgets.
- Optional confirmation before AMF submits the next composer prompt
  after a budget is exceeded.

Warnings appear as a dashboard badge/toast and a modal when the user
next enters or submits through AMF. AMF cannot reliably enforce a hard
stop when the user types directly into an embedded harness pane, so the
UI must call these **soft budgets**, not billing limits. Never kill a
running process or interrupt an in-flight tool call solely for budget
enforcement.

### 4. Resume, compact, and fresh-session rotation

Make restart behavior explicit and consistent across harnesses. When a
stopped session has provider context, offer:

1. **Resume:** continue the exact provider session.
2. **Compact and resume:** invoke the provider's supported compaction
   action, then continue.
3. **Fresh with handoff:** start a new provider session and seed it with
   a bounded structured handoff.
4. **Fresh:** start with no inherited conversation.

The same choices should be available from a leader command while
viewing a session. Harness adapters expose capabilities such as
`can_resume`, `compact_action`, and `can_report_context`; unsupported
actions are hidden rather than emulated badly.

Compaction is user-triggered because it costs a summarization step and
can discard details. AMF should preview the estimated context pressure,
identify the provider action it will send, and preserve the existing
session ID so the user can return if the compacted result is poor.

### 5. Structured, provider-neutral handoffs

Replace raw transcript export as the default fork context. A handoff
should contain:

```markdown
# Session handoff

## Goal and scope
## Settled decisions
## Work completed
## Files changed
## Commands and validation run
## Current failures or blockers
## Remaining tasks
## Recommended next action
```

Build the deterministic portions locally from feature metadata, git
status/diff names, plan/TODO state, and validation records. When a
semantic summary is useful, send only a bounded tail/selection from the
**exact selected provider session** through the utility-inference
profile. Cap both source material and generated output, show the
estimated utility cost before generation, and let the user edit the
handoff before launching.

Store handoffs in a gitignored `.amf/` or provider-neutral worktree
location rather than `.claude/`. Seed the new session's composer with a
short explicit instruction to read the handoff; do not assume a harness
will discover the file automatically.

The fork dialog should offer `None`, `Structured` (default), and `Full
transcript` (advanced) context modes. Full transcript export remains an
explicit escape hatch with a visible size estimate and hard cap.

### 6. Token-aware utility inference and caching

Route all AMF-owned headless calls through `HeadlessRunner` and attach a
task class:

- `summary`
- `change_reason`
- `walkthrough`
- `changeset_overview`
- `code_review`
- `plan_interview`
- `handoff`

Each task class can select a utility profile and input/output caps.
Default one-line session summaries to a local heuristic based on the
latest assistant/status text; model generation becomes an opt-in
fallback. Stop hardcoding Claude for summaries and change-reason jobs.

Persist model-generated utility results by a stable content hash that
includes task type, prompt version, relevant diff/transcript input,
model, and effort. Reopening an unchanged walkthrough or changeset
overview should produce zero additional model calls. A deliberate
"regenerate" action bypasses the cache.

Every utility loading view should show:

- Harness/model and effort profile.
- Estimated input size before launch.
- Configured maximum output.
- Actual tokens and cost afterward when reported.
- Whether the result was generated or served from cache.

The plan-mode interview must also use this policy. Put stable
instructions and repository context before changing answers to preserve
provider prompt-cache prefixes where supported, avoid resending
unchanged large files when a smaller repository summary suffices, and
cap accumulated Q&A by token size rather than characters alone.

### 7. Remove the Review Mode per-edit token tax

Stop requiring a note before every Edit or Write in
`ensure_review_claude_md`. Replace it with:

- Local changed-file and diff metadata collected by AMF.
- An optional single developer rationale per file or review round.
- Existing on-demand walkthrough generation when the reviewer wants a
  semantic explanation and no rationale exists.
- Persistent content-hash caching for generated walkthroughs and the
  changeset overview.

If an agent-authored implementation narrative is enabled, request it
once before review or feature stop and cap its length. Do not keep the
instruction in an always-loaded file when review mode is disabled.

### 8. Prompt, instruction, and tool-output hygiene

Extend the existing local prompt steering analyzer with a token estimate
and deterministic warnings for:

- Pasted full files that already exist in the worktree.
- Very large logs or test output.
- Repeated conversation/history pasted into a continuing session.
- Broad exploration requests without scope or acceptance criteria.
- Prompts that could reference `path:line`, a command, plan, TODO, or
  review comment instead of duplicating content.

Integrate the indicator with AMF's composer, prompt-library injection,
TODO launch, PR-comment fixes, and plan synthesis. Warnings are advisory
and require no model call.

Add an instruction-size audit in the debug/config view:

- Measure AMF-owned blocks injected into `CLAUDE.local.md` or
  `AGENTS.md`.
- List installed AMF skill names and description sizes.
- Shorten always-visible descriptions and move specialized guidance to
  on-demand/manual commands where each harness supports it.
- Add snapshot tests that enforce size budgets for AMF-owned persistent
  instructions.

Expose provider-native tool-output caps and compaction/pruning settings
through profiles where available. Add optional local output reducers for
common test/log commands only where the harness can safely support them;
always preserve a path to the full raw output for debugging.

### 9. Local efficiency analytics

Once the decision-facing snapshot is correct, persist sparse local
usage samples only when usage changes. Roll up or prune old samples so
the database does not grow with every sync tick. The dashboard can then
answer:

- Which active sessions have the highest recent burn rate?
- How much usage came from subagents?
- Which models/profiles are used for which tasks?
- How often did structured handoffs replace full-history resumes?
- How much utility work was served from cache or handled locally?
- Did cache-read ratios drop after an instruction/prompt change?
- What was the cost per feature, accepted review round, or completed
  TODO?

Recommendations must be explainable (for example, "three routine
one-file fixes used Deep; consider Balanced") and dismissible. Do not
automatically change a feature's profile based on analytics.

### 10. AI ecosystem update checks and capability registry

Models, effort controls, context windows, prices, CLI flags, usage-event
schemas, and provider defaults change independently and frequently. AMF
needs an easy way to discover drift without requiring a maintainer to
re-audit every harness manually or waiting for a broken session launch.

Add a provider capability registry with two layers:

- **Shipped knowledge:** the model aliases, effort/variant values,
  context limits, pricing metadata, launch flags, compaction actions,
  and transcript/event shapes known when the AMF release was built.
- **Observed installation:** installed harness version, locally probed
  help/capability output, models visible to the user's configured
  account where a provider offers a safe listing command, and the last
  successful check time.

Expose an **AI compatibility report** from the config/debug UI and a
CLI/doctor command. It should show, per harness:

- Installed and last-known-tested CLI versions.
- Configured, requested, and observed models.
- Supported model-selection and effort/variant values.
- Resume, compaction, headless, tool-output-limit, usage-reporting, and
  context-reporting capabilities.
- Known context window and price metadata, including its source and
  freshness date.
- Unknown flags, removed model aliases, new unrecognized effort values,
  or provider event fields AMF is currently ignoring.
- A copyable diagnostic report that omits credentials, prompts,
  transcript content, and other sensitive values.

Provide an explicit **Recheck AI capabilities** action. Local CLI/version
and `--help` probes may run automatically after a detected binary version
change, but model-catalog or documentation/network checks must be
user-triggered, cached, and allowed to fail offline. Never make a network
request or enumerate account models on every AMF startup.

The registry should distinguish three states instead of treating support
as a boolean:

- `Known`: verified by AMF's shipped knowledge or a successful probe.
- `ObservedUnknown`: reported by the installed harness/provider but not
  understood by this AMF version.
- `Unavailable`: explicitly absent from the installed version/account.

An unknown model or effort value should remain selectable through an
advanced/custom field, with a warning, so AMF does not block a newly
released capability while awaiting an AMF release. Conversely, AMF
should not silently rewrite a removed model alias or profile. Offer a
previewed migration and preserve the previous configuration until the
user accepts it.

Keep provider-specific parsing and probe commands behind the same
capability adapter used by launch and context rotation. Store the last
report locally so bug reports can say "this was last verified against
Codex X / Claude Y / opencode Z" and tests can use fixtures rather than
calling live provider services.

## Harness capability matrix

Capabilities must be detected against the installed CLI rather than
assumed forever. At the time this plan was written (2026-07-17):

| Capability | Claude | Codex | opencode | Pi |
| --- | --- | --- | --- | --- |
| Interactive model selection | Yes | Yes | Yes | Unverified |
| Reasoning effort/variant | Yes | Yes | Yes | Unverified |
| Resume exact session | Yes | Yes | Yes | Needs audit |
| Native compaction/config | Yes | Yes | Yes | Unverified |
| Local per-session usage source | Yes | Yes | Yes | No current integration |
| AMF headless utility runner | Yes | Yes | Yes | Limited/unverified |

Keep the adapter capability-driven. A provider upgrade that removes or
renames a flag should disable that control with a clear debug entry,
not prevent the session from launching.

## Progress

### P0 — Correctness and immediate waste removal

- [ ] Add observed provider/model metadata to usage snapshots.
- [ ] Replace the global-only price calculation with provider/model
      lookup and an explicit unknown-price state.
- [ ] Expose latest-turn usage and cache ratios from provider readers.
- [ ] Separate Claude parent and subagent usage before aggregation.
- [ ] Add usage-source/model-change debug logging from the remaining
      per-session usage backlog.
- [ ] Replace model-generated session summaries with a local default;
      route the fallback through `HeadlessRunner`.
- [ ] Remove the Review Mode instruction requiring a note before every
      edit and retain on-demand walkthrough behavior.
- [ ] Add tests proving mixed models do not use a single default price,
      a summary can be generated without a headless call, and Review
      Mode no longer requests per-edit notes.

Acceptance criteria:

- Mixed-provider token counts remain visible, but dollar cost is shown
  only when AMF has a matching price.
- The dashboard can show the latest turn separately from lifetime use.
- Routine session summaries consume no model tokens by default.
- Enabling Review Mode no longer adds an agent-authored tool call before
  every file edit.

### P1 — Efficiency profiles and launch controls

- [ ] Add configurable named efficiency profiles and built-in
      Economy/Balanced/Deep defaults.
- [ ] Add profile selection to feature creation, feature presets, and
      agent-session creation.
- [ ] Persist feature defaults and per-session requested settings.
- [ ] Add Claude, Codex, and opencode launch translations for model and
      effort/variant.
- [ ] Show selected profile, requested model, and observed model in the
      sidebar/debug view.
- [ ] Add a separately configurable utility-inference profile.
- [ ] Add the provider capability registry and versioned shipped
      knowledge used by profile validation.
- [ ] Add safe local probes for installed harness version, model/effort
      flags, and supported context/session actions.
- [ ] Add an AI compatibility report and explicit capability recheck
      action, with cached/offline behavior.
- [ ] Warn about stale aliases and observed-but-unknown values while
      preserving an advanced custom-value escape hatch.
- [ ] Add launch tests for every supported flag/config translation and
      graceful fallback when a CLI lacks the capability.

Acceptance criteria:

- Two sessions in one feature can use different profiles.
- Presets can choose a profile without embedding provider-specific
  arguments in the preset.
- Existing configurations migrate to Balanced/current harness defaults.
- A missing or obsolete optional flag does not make the feature
  unlaunchable.
- A user can recheck installed AI capabilities and see what changed
  without reading provider release notes or exposing session content.
- Network/account catalog checks never run implicitly at every startup.

### P2 — Context pressure, budgets, and rotation

- [x] Calculate provider-specific active-context estimates and record
      known model context windows.
- [x] Render context pressure in agent session rows, with direct/estimated,
      stale, warning, critical, and reset states.
- [ ] Render latest-turn usage and recent burn in the session row/sidebar.
- [x] Add configurable context-window-size and warning/critical percentage
      thresholds, global via `AppConfig` and a dedicated dashboard dialog
      (`w`, `src/app/context_settings.rs`, `src/context_tracking.rs`).
      Cumulative token/dollar usage thresholds and per-utility-call budgets
      are still open.
- [ ] Add dashboard/sidebar warnings and deduplicate repeated alerts.
- [ ] Introduce a capability-driven resume/compact/fresh dialog.
- [ ] Make normal restart semantics consistent across Claude, Codex,
      and opencode.
- [x] Add a leader command for fresh-session rotation (`Ctrl+Space` then
      `F`, `src/app/handoff.rs`): starts a new agent session in the same
      feature/worktree, seeded with a plan-plus-changed-files prompt.
      Compact rotation, and binding the prompt to the structured handoff
      schema in P3 below, are still open.
- [ ] Add tests for unknown context limits, threshold crossings, exact
      versus inferred sources, and unsupported compaction.

Acceptance criteria:

- A user can identify a high-context/high-burn session before sending
  the next prompt.
- Thresholds never terminate an in-flight agent operation.
- Every supported harness presents deliberate resume semantics instead
  of relying on an accidental provider-specific default.
- Compaction/fresh actions are never triggered silently.

### P3 — Structured handoffs and fork repair

- [ ] Define the provider-neutral handoff schema and gitignored storage
      location.
- [ ] Collect deterministic feature/git/plan/TODO/validation fields
      without a model call.
- [ ] Bind optional transcript material to the exact selected provider
      session.
- [ ] Add bounded semantic summarization through the handoff utility
      profile.
- [ ] Add handoff preview/edit and `None` / `Structured` / `Full`
      choices to fork and fresh-session rotation.
- [ ] Seed the new session explicitly with the selected handoff.
- [ ] Keep raw transcript export as an advanced, size-capped option.
- [ ] Add cross-provider tests and large-transcript fixtures proving the
      default path is bounded.

Acceptance criteria:

- Structured is the default fork context and has a configured maximum
  input and output size.
- Forking a selected session never silently uses another session's
  transcript.
- Claude, Codex, and opencode can receive the same handoff format.
- A new agent is explicitly told where the handoff is and why it should
  read it.

### P4 — Utility inference and persistent caching

- [ ] Inventory every AMF-owned headless call and assign a task class.
- [ ] Route direct `ClaudeLauncher::run_headless` callers through
      `HeadlessRunner` or a local implementation.
- [ ] Add per-task input/output budgets and preflight estimates.
- [ ] Add a persistent content-hash cache for walkthroughs, changeset
      overviews, handoffs, and other safe deterministic inputs.
- [ ] Show actual/estimated usage and cache state in loading/result UI.
- [ ] Reorder and bound plan-interview prompts for cache-friendly stable
      prefixes.
- [ ] Add tests proving an unchanged cached task launches no process and
      that regeneration bypasses the cache.

Acceptance criteria:

- No AMF utility silently hardcodes Claude when a local or configured
  harness path exists.
- Reopening an unchanged review walkthrough or overview costs zero
  additional tokens.
- Every launched utility task has explicit input/output limits and a
  visible selected profile.

### P5 — Prompt, instruction, and output hygiene

- [ ] Add local prompt-size estimates and large-paste/repeated-context
      warnings to the steering analyzer.
- [ ] Integrate warnings with composer and AMF-generated prompt entry
      points without blocking submission.
- [ ] Audit and minimize AMF-owned always-loaded instructions.
- [ ] Audit skill descriptions and use provider-appropriate on-demand
      visibility where supported.
- [ ] Add instruction/skill size budgets and regression tests.
- [ ] Expose safe provider-native tool-output, pruning, and compaction
      controls in efficiency profiles.
- [ ] Add opt-in local reducers for supported large-output workflows.

Acceptance criteria:

- AMF can warn about an avoidable large prompt without making a model
  call.
- AMF-owned persistent instructions have tested size ceilings.
- Output reduction never removes access to the full raw command output.

### P6 — Budgets and efficiency analytics

- [ ] Persist sparse usage deltas/model segments with retention and
      roll-up rules.
- [ ] Add feature/session soft token and cost budgets.
- [ ] Add recent-burn, subagent, cache, profile, handoff, and utility
      cache metrics.
- [ ] Add a local efficiency dashboard and exportable summary.
- [ ] Add explainable, dismissible profile recommendations.
- [ ] Establish a dogfood baseline before setting percentage reduction
      targets.

Acceptance criteria:

- Historical samples grow with usage changes, not every sync tick.
- Users can identify the sessions and AMF utility tasks responsible for
  recent usage.
- Recommendations never change a profile or stop a session
  automatically.

## Suggested release slices

### Slice 1 — Honest accounting and free savings

Ship P0. Correct mixed-model cost reporting, expose last-turn data,
make session summaries local, and remove the per-edit review note tax.
This delivers savings before adding new workflow UI.

### Slice 2 — User-controlled efficiency

Ship P1 plus the context indicator and warning portion of P2. Users can
choose model/effort intentionally and see when a session is becoming
expensive.

### Slice 3 — Context lifecycle

Finish P2 and P3. Deliver deliberate resume/compact/fresh behavior and
bounded structured handoffs across supported harnesses.

### Slice 4 — AMF-owned inference discipline

Ship P4 and P5. Centralize utility calls, cache safe results, add prompt
hygiene, and minimize persistent instruction/tool-output overhead.

### Slice 5 — Learning loop

Ship P6 after dogfooding provides a trustworthy baseline and confirms
which recommendations are useful rather than noisy.

## Success criteria

- No AMF workflow includes a complete transcript by default.
- No paid AMF utility call lacks an explicit model/profile and bounded
  input/output policy.
- Repeating an unchanged cacheable utility action launches no model
  process.
- Model-aware costs are correct or explicitly unknown; AMF never prices
  one provider as another silently.
- Latest-turn/context pressure is visible separately from cumulative
  usage.
- Review Mode no longer requires agent-authored prose before every edit.
- New sessions can deliberately resume, compact, or start fresh with a
  bounded handoff across every harness that supports the action.
- AMF exposes a cached compatibility report for installed harness
  versions, models, effort controls, context metadata, and unknown
  provider changes, with a user-triggered recheck action.
- AMF adds no unbounded transcript scans or per-feature background
  workers to the normal refresh path.
- After a dogfood baseline exists, release notes report measured changes
  in context size, utility cache hits, full-history resumes, and cost per
  completed feature rather than claiming an unverified percentage
  saving.

## Risks and mitigations

- **Lower-effort models can cause retries.** Keep profiles explicit,
  allow one-key escalation, and measure completed-work outcomes rather
  than optimizing raw token count alone.
- **Context estimates differ by provider.** Label estimates and source
  confidence; do not compare percentages when the model limit is
  unknown.
- **Compaction can omit important decisions.** Require confirmation,
  preserve the original provider session, and offer an editable
  structured handoff as the safer fresh-session path.
- **Pricing changes over time.** Make the catalog overrideable and show
  unknown/stale metadata rather than promising live billing accuracy.
- **Provider flags change.** Probe capabilities, isolate CLI translation
  in adapters, and degrade without blocking launch.
- **Live capability checks can be slow, private, or account-specific.**
  Keep network/model-catalog checks explicit, redact diagnostic output,
  cache results, and use local version/help probes by default.
- **Usage history can grow indefinitely.** Persist sparse deltas, roll up
  older records, and make retention configurable.
- **Prompt warnings can become annoying.** Keep them advisory,
  thresholded, deduplicated, and dismissible per project.

## Open questions

- Should the selected efficiency profile live on `Feature` directly or
  should `Feature` reference only a named config profile that may change
  later? A stored resolved snapshot is safer for reproducibility, while
  a name is easier to administer globally.
- Which built-in models should Economy/Balanced/Deep select? Stable
  provider aliases reduce churn but may change behavior; explicit model
  IDs are predictable but age quickly.
- Should structured handoffs use a model by default or begin as a fully
  local deterministic summary with an optional "improve" action?
- Where should provider/model price updates come from? The initial
  design should remain local/configurable rather than requiring network
  access at startup.
- Which sources should AMF trust for model catalogs, effort values,
  context windows, and deprecation notices: installed CLI output,
  provider APIs, official documentation metadata, or a versioned AMF
  catalog? The report should display provenance rather than merge
  conflicting values silently.
- Should budget state be per AMF `FeatureSession`, per provider session,
  or both when multiple AMF windows intentionally resume the same
  provider session?
- How should AMF estimate active context for providers whose transcript
  reports cumulative billing tokens but not the post-compaction context?
- Which direct-pane prompt paths can AMF observe reliably enough to show
  a pre-submit budget warning without interfering with terminal input?
- Should utility caches live in SQLite, a gitignored worktree cache, or
  both? SQLite is easier to prune; worktree-local storage is easier to
  inspect and move with a feature.

## Reasoning / when to build

Build P0 before adding more usage visualizations. A more detailed cost
dashboard is not useful if mixed models are priced incorrectly or if it
only shows cumulative totals. The local-summary and Review Mode changes
also remove concrete waste with little dependency on new UI.

Then build profiles and context warnings before automatic suggestions.
They give the user direct control and generate the trustworthy usage
segments needed to evaluate later recommendations. Structured handoffs
should follow closely: they are the safest way for AMF to help users
leave oversized contexts without discarding the work that matters.

Analytics comes last. Its value depends on correct model attribution,
per-turn deltas, explicit profiles, and observable handoff/cache events.
Until those exist, AMF should avoid making confident claims about which
workflow is "more efficient."
