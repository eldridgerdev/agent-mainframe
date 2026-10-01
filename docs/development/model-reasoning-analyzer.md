# Model and reasoning analyzer: capability inventory

Inventory and first-release implementation checked on 2026-09-29, with Claude
Code discovery and research added on 2026-09-30 and additional plan-review entry
points, including Expert review advice, on 2026-10-01, against
`AMF_PLAN.md`. The headless inventory includes the subsequent reasoning-picker
integration from main. Live application to existing sessions remains outside the
verified launch scope.

## Configuration and availability

`AgentKind::ALL` lists Claude Code, Codex, OpenCode, and Pi. A configured harness
is the intersection returned by `App::allowed_agents_for_repo`: effective
`ExtensionConfig.allowed_agents` (project overrides global) and
`ProjectStore.available_harnesses`. An unset/empty configuration expands to the
existing default harness list before taking the intersection. An empty resolved
intersection means **no eligible harnesses**. Preferred agents and feature
presets select defaults; they do not grant access to a harness or model.

`TmuxOps::check_harness_available` delegates to `App::check_harness_available`.
It checks `--version`; Claude resolves a working binary, including fallback
installed versions. This proves CLI readiness, not authentication, provider
configuration, model entitlement, or effective reasoning policy. Headless
readiness is a separate contract: Codex checks `exec --help`; Pi's interview
selector checks the required isolation flags. `HeadlessRunner::select_for_interview`
prefers the requested harness and uses a stable fallback order. It is not
restricted to `allowed_agents_for_repo` and does not verify authentication.
The analyzer applies the configured harness intersection to runner selection
and to recommended options.

Capability probes belong in workers, outside the event loop. Avoid paid runs
to test availability: trials are deferred. A failed or ambiguous access check
must leave explicit model access unknown/unavailable. CLI help was inspected for
locally installed Claude, Codex, and OpenCode; Pi is not installed here.

## Harness inventory

### Claude Code

`app::ai_review::model_pick_rows` currently offers `sonnet`, `opus`, `haiku`,
`fable`, and a custom model. These aliases are recognized syntax; their presence
is not account-specific access evidence. Defaults and pinned model IDs depend on
account/provider configuration and managed restrictions. Aliases can change the
underlying model over time.

The installed CLI advertises `--model` and `--effort`. Effort support depends on
the concrete model; managed/organization caps can silently reduce a requested
level in structured output. The analyzer needs an effective model/access and
effort-capability report before admitting an explicit combination. Do not infer
supported levels from a tier alias or reuse one model's levels for another.
The analyzer uses a bounded SDK-control discovery process in the same workdir
and the same resolved Claude binary as launch. Initialization supplies canonical
`resolvedModel` IDs and per-model `supportedEffortLevels`; aliases, unresolved
defaults and disabled rows are excluded. Each exact ID must pass `set_model`,
then `get_settings` must report that exact applied model and effective effort.
A temporary explicit `high` effort request reveals any model-specific cap;
the offered low/medium/high levels are the intersection of supported levels
and that applied cap. Higher levels remain outside the current tradeoff contract.
Models rejected by the SDK or substituted with another ID stay unavailable.

Discovery sends no user messages or generation requests. It disables tools,
hooks, Chrome and skills, uses an empty strict MCP configuration, and disables
session persistence while preserving model settings, authentication and policy
resolution. Model switching affects only the throwaway discovery process.
It requires authenticated first-party Anthropic metadata; custom providers,
model override maps and forced effort/body environment overrides remain
unverified. Older CLI protocols lacking canonical IDs or applied-setting
inspection fail closed. The entire probe, including cancellable binary readiness checks, shares a
30-second deadline. Protocol behavior was checked with Claude Code 2.1.285.

`TmuxOps::launch_claude` accepts `extra_args`, quoted individually by
`TmuxManager`; this can carry `--model <id>` and `--effort <level>` without
editing settings files. Existing permission, review, Chrome, Remote Control,
hook and resume arguments must be preserved. Headless calls accept model and
reasoning through `ModelSel`; the analyzer leaves its runner's settings unspecified.

Sources: [Claude model configuration](https://code.claude.com/docs/en/model-config),
[CLI reference](https://code.claude.com/docs/en/cli-reference).

### Codex

`codex_config::known_models` reads `~/.codex/models_cache.json`, then the legacy
`tui.model_availability_nux` table. `spawn_cli_catalog_probe` runs
`codex debug models` in a worker. Catalog entries contain `slug`, `visibility`,
`supported_reasoning_levels[].effort`, and `default_reasoning_level`.
Only `visibility = "list"` is eligible picker metadata. NUX/configured names
record historical/configured choices, not current access or capability.
`supported_in_api` describes API support and must not be substituted for
eligibility under the user's configured provider/account.

The analyzer parser retains model-specific levels and an explicit access
assessment supplied by discovery. A cache read remains **unknown access**;
even a fresh CLI catalog must be associated with the effective provider,
account, workdir and restrictions before admitting choices. Missing reasoning
metadata permits no explicit level. A default level is not a list of supported
levels. Client/catalog versions and freshness need assessment in the evidence
contract task.

Interactive launch already accepts quoted `extra_args`. Use `--model <slug>`
and `-c 'model_reasoning_effort="<level>"'` with the catalog's exact level.
Preserve notify, working-directory, mode/sandbox, watcher and resume arguments.
`configured_model()` reads only the top-level user config; it does not resolve
profiles, project or managed defaults and cannot establish effective identity.
The existing filesystem readers use `~/.codex` directly. Analyzer discovery must
match the launching CLI's effective home, profile, provider and project context,
including `CODEX_HOME` and configured catalog overrides.

Sources: [Codex configuration reference](https://developers.openai.com/codex/config-reference/),
installed `codex --help` and `codex debug --help`, and catalog field inspection.

### OpenCode

AMF's existing picker has Default/Custom, without a discovered catalog.
OpenCode identifies models by `provider/model`; reasoning variants are
model/provider-specific and can be configured or disabled. Effective catalogs
are scoped to the project. The CLI offers `models` (and metadata via
`--verbose`), but public catalog membership alone does not prove the provider
is connected. The server's `/provider` response separates its catalog from
connected provider IDs; an adapter would need effective enabled models,
connected providers and variant metadata for the launch's workdir.

The installed interactive CLI advertises `--model` but no `--variant`;
`opencode run` documents `--variant`. V2 documents `provider/model#variant`.
These interfaces must not be mixed without checking the installed version.
Current `TmuxOps::launch_opencode[_with_session]` accepts no extra arguments;
therefore **no explicit model or variant is currently applicable through AMF's
interactive launch boundary**. HeadlessRunner can pass an explicit model and
reasoning as `--variant`, but this is not effective provider/variant discovery.
Extending interactive launch requires that discovery contract first.

Sources: [OpenCode CLI](https://opencode.ai/docs/cli/),
[server providers](https://opencode.ai/docs/server/),
[V2 model references](https://opencode.ai/v2/docs/models), installed CLI help.

### Pi

AMF's existing picker has Default/Custom and no catalog adapter. Pi exposes
`--list-models`, provider-qualified model IDs, `--provider`, `--model`, and
`--thinking`. Levels are model-specific; a requested unsupported level may be
clamped. The provider registry/auth state, scoped model settings, custom models
and extensions affect effective choices. A global list of level names or a
model's `reasoning` boolean is insufficient to prove each combination.

Current `TmuxOps::launch_pi` accepts no extra arguments: **explicit model and
thinking choices cannot reach interactive execution today**. HeadlessRunner
accepts a model and checks isolation flags for interviews, but has no thinking
parameter. The future adapter must preserve exact provider/model identity and
effective thinking levels instead of relying on fuzzy CLI matching. Validate
against installed-version capabilities; upstream documentation is not a local
readiness check.

Sources: [Pi CLI](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/cli.md),
[Pi model selection](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md),
[CLI argument validation](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/cli/args.ts).

## Settings handoffs inside AMF

| Boundary | Current behavior | Analyzer treatment |
| --- | --- | --- |
| `PreparedFeatureLaunch` / `PendingPlanLaunch` | Retain harness, mode, accepted plan and transient analyzer selection | Carry selected settings and target identity through acceptance/resource confirmation |
| `Feature` / `FeatureSession` | Persist harness/session kind and resume/token source identity; no explicit model/reasoning history | Do not treat defaults or older sessions as historical model evidence |
| `finish_feature_launch_*` / `ensure_feature_running_*` | Build harness-specific startup args and start saved sessions | Revalidate the transient selection immediately before initial launch; preserve rollback, mode flags and resume behavior |
| `launch_agent_session_window` and session restart | Claude/Codex extra arguments; OpenCode/Pi fixed launch signatures | Apply selected arguments only to the shipped initial-agent path through `TmuxOps` |
| `HeadlessRunner` | All four accept `model`; Claude/Codex/OpenCode accept explicit reasoning through `ModelSel`; restricted/read-only contracts differ | Analyzer runner selection/fallback respects configuration, leaves its runner's settings unspecified and never recursively calls analysis |
| Expert/AI Review model pickers | Manual model and harness-supported reasoning selections | Expert picker `m` offers scoped advice, then returns to manual selection; picker rows alone are not proof of account access |

## Implemented eligibility boundary

`src/model_options.rs` is the pure boundary used by the analyzer workflow.
`EligibleOptions::new` intersects resolved configured harnesses with current
harness and model availability, installed model/reasoning controls, exact
model-specific levels and the chosen AMF launch path. It excludes missing,
unknown, unavailable, malformed and contradictory capability reports. Identical
duplicates are deduplicated; conflicting reports are not merged into broader
access. Unresolved default models are not comparative options. `reasoning = None`
leaves the harness setting unspecified and does not claim a concrete level.
OpenCode `#variant` identifiers and colon-bearing Pi identifiers are excluded
as ambiguous implicit reasoning selections until a resolver/launch seam can
carry and validate those controls explicitly.

Choices receive stable IDs derived from the complete harness/model/reasoning
tuple. Selection accepts only an ID in the eligible set. Revalidation against
a newly constructed set rejects changed configuration, unavailable harnesses,
removed levels and changed launch support. Interactive arguments are returned
only after revalidation and only for a supported path; no process or settings
write occurs in this module. Claude/Codex use their existing extra-argument seam.
OpenCode/Pi interactive overrides remain excluded until their launch seams are
implemented. Initial-launch selections target interactive implementation.
Expert-review advice uses the headless path: discovered per-model effort levels
must also be expressible by `ReasoningLevel`/`ModelSel` for that harness. Unknown
levels that the headless seam would drop are excluded. This does not establish
OpenCode/Pi access; they still lack verified discovery. Advice never changes the
analyzer runner's own settings.

The Codex name picker reuses the catalog parser while keeping parsed access
unknown for analyzer purposes. The plan-review workflow uses live discovery
and repeats eligibility before application and launch. Tests exercise the
boundary with simulated effective capability reports for all four harnesses,
plus scoped workflow and launch mocks; they do not execute paid trials.
Target identity and freshness checks are described below.

## Evidence inventory and first release

Session usage (`token_tracking.rs`, `db/token_cache.rs`) is cumulative across
turns, sometimes inferred, and does not record model, effective effort, phase,
task boundary or quality. Pi has no token provider. Missing session usage stays
unknown; individual historical counters normalized to zero cannot establish
that a missing field actually measured zero. Do not import these totals.
Headless events expose optional token counts for a single pass; missing counters
remain unknown. Their elapsed wall time includes startup and may include retries;
neither records comparative task quality. Plan preflight records include a
requested model and plan fingerprint, but token estimates are heuristics and
`completed` means execution completed, not a successful implementation. Review
caches are tied to a branch/head, fix-cost deltas to a review comment, and final
review checks to a particular diff/command. None is a model-attributed benchmark.
These signals remain useful within their existing planning/review UI; no v1
analyzer score, aggregation or historical measurement is derived from them.

| Candidate entry | Context / handoff | v1 |
| --- | --- | --- |
| New feature full/Quick Plan review | Reviewed plan, project identity, workdir, deferred launch; advice targets implementation | `m`, optional |
| Before interview | Brief only; targets planning, separate headless configuration | Deferred |
| On-demand Full/Quick Plan review | Reviewed implementation plan and stable feature destination; accept writes plan and offers a kickoff handoff | `m`, advice only for the feature harness |
| Host-feature TODO Plan review | Reviewed implementation plan and resolved host/TODO; accept writes a separate plan and starts a TODO session/composer | `m`, advice only for the host harness |
| New-feature TODO Full/Quick Plan review | Reviewed implementation plan and prepared destination; accept creates feature and links TODO | `m`, selection applies to initial implementation launch |
| Direct TODO spawn | Destination/reservation/composer without a reviewed plan | Deferred |
| Expert plan-review model picker | Draft plan, brief, answers, repository/reference context and resolved reviewer; separate headless critique | `m`, advice only for the reviewer; return to AMF picker |
| AI/Final/PR review | Diff/head and review task; separate worker/launch | Deferred |
| Existing agent session | Feature name/summary and effective plan when present; advice for the current harness | Dashboard session `m` or pane leader `B`; view-only |

A selection affects the initial implementation agent launch only. It returns to
plan review; accepting the plan still controls launch and resource confirmation.
Editing the plan invalidates the choice. Later independent sessions/restarts keep
their existing configuration semantics. Cancellation launches no implementation.

The `m` action also covers on-demand Full/Quick Plan review and TODO Plan
review. Outside the Expert picker, advice targets **implementation of the
reviewed plan**. Existing-feature and host-feature TODO plans
are view-only, restricted to the destination feature's configured harness.
Accepting an on-demand plan still writes it and offers the existing kickoff
handoff. Accepting a host TODO plan still writes a separate TODO plan and starts
its own session/composer, using the existing reservation and rollback behavior.
The dialog explains these scopes and offers no Apply action for either path.
The user changes the harness setting with its own picker.

New-feature TODO plans can apply a selection through the same initial-launch
boundary as other new plans, including fresh discovery after resource approval.
Evidence is loaded for the resolved **destination project/repo**, even when the
TODO originates in a global list or another project. Advice reads the live TODO
and owning list from the DB and snapshots task content, work status/links and
owner identity along with the resolved review target. Unrelated sibling edits,
list timestamps/scratchpad changes and reordering do not invalidate the choice.
Edits, completion, deletion, moves, list/host changes, destination changes and
changed configuration reject pending results. The source snapshot also travels
with a selection, so changing a TODO between application and plan acceptance
invalidates it. Cancellation leaves the plan and reservation intact.
Successful launch retains existing TODO feature/session links; retries preserve
the same destination and only clean up the tmux session created by the attempt.
No preference, observed effective setting, usage measurement or generated claim
is persisted by this increment.

The Expert model picker has its own `m` entry. Advice targets **review of the
draft plan**, using the resolved reviewer harness rather than the implementation
harness. It carries the brief, interview questions/answers and draft plan; the
worker reads the same bounded repository context used by critique and bounded
excerpts of explicitly attached reference documents. Reference excerpts are
labelled when truncated, and full bounded-file fingerprints detect edits beyond
the excerpts. Missing, invalid or oversized references fail closed. Reading
references creates no staged copies or workflow state. Interview, TODO,
destination, reviewer or configured-harness changes invalidate pending results;
the worker rereads repository/reference context before returning advice.
Cancellation restores the original picker, including its custom model buffer
and selected effort. Custom-model typing keeps `m` as text. The dialog offers no
Apply action: choose model/effort in the existing AMF picker and confirm review
separately. No critique starts or preference is persisted through this advice.

Before-interview planning, direct TODO spawning and AI/Final/PR review entry
points remain deferred. They need planning or diff/head-specific targets;
interactive implementation arguments cannot configure those workers. Live session
application, persistent requested/effective stage settings,
outcome attribution/comparability and OpenCode/Pi discovery/launch adapters remain
separate capability work. Paid trials remain optional, explicitly opt-in work.

For an existing session, the analyzer only offers settings for that session's
verified harness. The worker reads the effective plan alongside feature name
and summary, then checks that task context again before accepting a result.
Changing or deleting the session, feature or configured harnesses invalidates
pending advice. This path does not reconfigure or restart an active harness;
interactive startup arguments cannot change a process that is already running.
The session view directs the user to the harness's own model picker. Live
application and the unidentified “all 3” existing-session actions remain
separate work until AMF has a verified control boundary for them.

The advice dialog presents model, effort and focus in a highlighted settings
table. The default view keeps tradeoffs and unknown measurements concise;
`s` shows the full attributed research, URLs and freshness dates. `PgUp/PgDn`
scrolls that research while the choices and actions stay visible. On small
terminals the settings table follows the selected row.

## Research contract

V1 persists repository-reviewed research notes only, with stable source ID,
HTTPS primary-source URL, checked timestamp, expiry and exact paraphrase. The
first note is OpenAI's [reasoning guide](https://developers.openai.com/api/docs/guides/reasoning),
checked 2026-09-29, expiring after 30 days. It supports qualitative low/medium/high
effort tradeoffs **within one Codex model**, not a measured ranking between models or an
implementation-quality prediction. A second note from the official
[model-selection guide](https://developers.openai.com/api/docs/guides/model-selection)
qualifies GPT-6 Luna/Sol/Astra task roles and permits alternatives between those
exact, live-discovered models only when both choices cite it as well as effort
guidance. Version identities were checked against their official
[Luna](https://developers.openai.com/api/docs/models/gpt-6-luna),
[Sol](https://developers.openai.com/api/docs/models/gpt-6-sol) and
[Astra](https://developers.openai.com/api/docs/models/gpt-6-astra) model pages.
A separate version-specific note from the official
[GPT-6.1 Sol model page](https://developers.openai.com/api/docs/models/gpt-6.1-sol),
checked 2026-09-30 and expiring 2026-10-30, qualifies its complex-coding and
professional-work role. It permits GPT-6.1 Sol alternatives when that choice
cites this note and effort guidance, and each other model cites its own applicable
model guidance and effort guidance. Earlier Sol research does not automatically
apply to a new version. Discovery already accepts exact new model IDs; research
never establishes account access. The analyzer selects up to three settings,
so a researched, available model is considered but need not appear in every result.
New effort levels have unknown tradeoffs.
Two Anthropic notes, checked 2026-09-30 and expiring 2026-10-30, use the official
[effort guide](https://platform.claude.com/docs/en/build-with-claude/effort) and
[Claude Code model configuration](https://code.claude.com/docs/en/model-config).
Effort guidance applies to documented exact Claude versions at verified
low/medium/high settings. Model-role guidance covers Sonnet 5.5, Opus 5.5,
Fable 5.1 and Fable 5. Haiku lacks a documented explicit effort control and
therefore does not qualify for these model-and-effort recommendations.
Anthropic evidence cannot justify a Codex choice, or vice versa. The analyzer
prompt lists applicable evidence IDs per option. Effort names describe
qualitative priorities and do not establish an equal scale across models.
Maintainers must read the source and update the note/version/date together;
loading or saving never refreshes it. Stale notes produce insufficient evidence.
The pure validator requires an exact match to reviewed provenance, freshness,
eligible option IDs and relevant citations. Generated output can select IDs and
speed/balance/depth, but cannot supply prose, numbers, URLs or measured evidence.
Model fit is a qualified analyzer judgment; displayed tradeoffs come from the
trusted note. Actual plan performance, duration and tokens remain unknown.

The project-scoped table is independent of feature rows, keyed by stable project
ID **and** repository path because AMF's global DB serves many checkouts. Existing
session results are not backfilled. Trials and future attributable observations
need a separate producer contract: exact task/phase/fingerprint, effective
harness/provider/model/effort, run boundaries, optional counters, outcome/check
provenance and comparability rules. They cannot be obtained by saving an analyzer
answer. DB failures are analysis errors, not insufficient evidence.

## Discovery and launch verification

The v1 discovery adapter owns a bounded, cancellable Codex app-server process
without creating threads or turns. It initializes, reads account authentication,
requests picker-visible `model/list` with per-model efforts, reads effective
provider configuration, and checks managed requirements. Protocol errors are
errors; custom providers, unauthenticated accounts and managed requirements
that this adapter cannot verify yield no eligible options. Large paginated
catalogs fail closed. Cached catalogs and bundled catalogs are never promoted.
See the official
[app-server protocol](https://learn.chatgpt.com/docs/app-server). The Claude
adapter described above also verifies access and effective effort caps. A
failed harness probe does not suppress verified choices from another configured
harness; when no usable capability report exists, protocol failures remain
retryable errors. OpenCode/Pi lack discovery and an interactive override seam,
so those options stay unverified.

Discovery is repeated when applying advice and after any resource confirmation
before launching. Target checks cover stable project identity/repo, reviewed
plan, interview key, workdir and prepared launch context. Runner fallback is
restricted to the configured harness intersection. Each request owns cancellation
and a generation; closed or stale results cannot apply settings. DB errors,
protocol errors, prompt validation errors and runner failures are retryable.
The analyzer has no numeric prediction fields and persists no generated output.

Selected arguments are appended to the existing launcher overrides for the
initial agent. Other sessions use their usual settings. Launch retries use a
stable feature ID and clean up only tmux sessions this attempt created; they do
not recreate a worktree or another feature row. Deleting or changing a saved
retry target invalidates that retry. Returning to review preserves the plan.

Plan-review expansion validation on 2026-10-01 passed 101 focused model/analyzer
checks plus plan, TODO, feature-session and PR-review suites (74/107/87/93 tests).
The locked build, full workspace suite, formatting and strict workspace/all-target
Clippy passed. The full suite passed 3,051 library tests plus seven GUI tests on
an isolated tmux socket with eight parallel threads; the existing live-GitHub test
remained ignored by default. Mocked workflows verified source/destination
isolation, stale TODO/feature rejection, resource-confirmation revalidation,
initial launch arguments, TODO links and retry behavior. No paid trials ran.

Expert-review expansion validation on 2026-10-01 used stable Rust 1.99. Focused
model/analyzer and Expert-picker checks, plan/TODO suites and the locked build
passed. The full parallel workspace suite passed 3,061 library tests and seven
GUI tests; one existing live-GitHub test remained ignored by default. Additional
coverage verifies reviewer/implementation separation, picker restoration,
reference/repository edits (including changes beyond reference excerpts), custom
model typing and TODO list changes unrelated to the selected task. Formatting
and strict workspace/all-target Clippy passed with warnings denied; the two
existing-main Rust 1.99 lint failures were fixed without suppressions. Seven
asserted screenshot frames use isolated plans and mocked discovery/responses;
no paid agents, critique runs or trials were launched by the scenario.
