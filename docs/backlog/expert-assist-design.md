# OPT-01: Expert Assist design and feasibility

- **Status:** The original manual prototype is complete but its PR was closed for
  reconsideration. Runner, owned-job, persistence, evidence, and prompt
  foundations remain reusable; the recommended product direction is now the
  automatic plan-preflight investigation in
  [the follow-up placement document](expert-assist-automatic-consultation-investigation.md).
- **Evidence date:** 2026-09-09.
- **Code inspected:** `b04c5709e8fb9412d2b89a08c01659fc3bd1a2e7` in the
  `agent-mainframe-token-minning` worktree.
- **Approved scope:** [AMF_PLAN.md](../../AMF_PLAN.md).
- **Option reference:** [Premium-model efficiency options, OPT-01](premium-model-efficiency-options-plan.md#opt-01-expert-assist).

## 1. Decisions and evidence boundaries

### Settled decisions

Expert Assist begins with a user request from an existing agent session. A
bounded headless expert gives advice, proposes a patch, or requests missing
evidence. The requesting implementer applies changes and validates them.
AMF stages an editable handoff. The user can **Send**, **View/edit**, or
**Dismiss**; inspecting or editing is optional, and delivery is never automatic.

The objective is lowest total monetary cost per accepted change at comparable
quality, even if total tokens increase. No harness/model pairing, numerical
budget, or billing basis has been approved. Feature creation is a possible
future configuration surface, not a committed entry point or permission mode.
This investigation does not implement Expert Assist or change those decisions.

After the investigation, the user authorized beginning the prototype. The
capability matrix below records the inspected commit; subsequent implementation
progress appears in section 10 and the plan's separate prototype checklist.

### Verified capabilities

“Verified” below means inspected Rust command construction, parsing, and
existing tests, not a live provider restriction test or a paid experiment.
CLI version, authentication, effective configuration, and actual model support
still need runtime validation. Source comments are evidence of intent; command
arguments establish what AMF actually requests.

| Foundation | Inspected evidence | Consequence |
| --- | --- | --- |
| Headless jobs | [`headless.rs`](../../src/headless.rs): `run`, `run_read_only`, `run_with_progress` | Model selection and policy builders exist, but progress always selects `command_for(harness, false)`. |
| Stable ownership | [`project.rs`](../../src/project.rs): `Feature.id`, `FeatureSession.id`, session kind/window and optional usage source | Bind to IDs, not selection indices or display names. A persisted launch generation is an extension. |
| Staged handoff | [`learning.rs`](../../src/app/learning.rs): `learning_escalate`; [`compose.rs`](../../src/app/compose.rs): `open_compose_seeded` | Learning creates/reuses a separate session. Seeded compose targets the current view and replaces draft text while keeping its images. Neither behavior satisfies Expert Assist unchanged. |
| Pre-call notice | [`precall.rs`](../../src/app/precall.rs): `PrecallAction`, `precall_gate`, `dispatch_precall` | User can continue without inspecting. Redispatch resolves the prompt again; consultation revision must remain bound to the notice. |
| Prompt configuration | [`prompts/mod.rs`](../../src/prompts/mod.rs), [`resolve.rs`](../../src/prompts/resolve.rs) | Stable prompt IDs and feature/project/global/built-in resolution can be reused. |
| Persistence | [`db/mod.rs`](../../src/db/mod.rs), [`migrations.rs`](../../src/db/migrations.rs), [`learning.rs`](../../src/db/learning.rs) | Global SQLite store and domain modules exist. Consultation records do not. |
| Process cleanup | [`headless.rs`](../../src/headless.rs): `LeasedChild`; [`procs.rs`](../../src/resources/procs.rs) | Dropping a leased child terminates its process tree in a background thread. Blocking runner functions use raw children and expose no cancel handle. |
| Concurrency | [`limits.rs`](../../src/resources/limits.rs): `HeadlessLease` | Counts in-flight runs; acquiring a lease increments a counter and does not itself enforce admission. |
| Usage/pricing | [`token_tracking.rs`](../../src/token_tracking.rs): `TokenPricingConfig`; `HeadlessUsage` | Optional counters and a single pricing table are insufficient for complete mixed-model cost attribution. |
| Presets | [`extension.rs`](../../src/extension.rs): `FeaturePreset` | Harness selection exists; model and effort fields do not. |
| Transcript export | [`transcript.rs`](../../src/transcript.rs): `find_latest_transcript`, `export_transcript_markdown` | Latest transcript by workdir is Claude-specific and can select the wrong session; it is not an Expert Assist evidence resolver. |

### Recommendations and deferred work

Sections 3–9 specify a recommended design, not newly approved product decisions.
Start with the user's existing implementer and a separately configured expert;
use deterministic evidence, one explicitly initiated round at a time, and
separate packet and repository-read profiles. Prototype cancellation, safe
delivery, and accounting before choosing models or claiming economic value.

Defer automatic escalation, automatic extra rounds, feature-creation placement,
model routing, paid summarization, repository indexing, patch application,
automatic validation by the expert, and a general usage-accounting rewrite.
Numerical limits and retention are proposed for a pilot in section 5, not
measured optima or changes to the approved plan.

## 2. Harness capability matrix

The matrix describes the adapters at the inspected commit. All four append
`--model <value>` when supplied. An omitted value inherits harness defaults;
Expert Assist should require an explicit value for reproducible comparisons.
No common effort option is wired through `HeadlessRunner`.

| Harness | `restricted=true` | `run_read_only` | Structured progress | Usage currently extracted |
| --- | --- | --- | --- | --- |
| Claude | `--safe-mode --tools ""`; adapter intends no tools and suppresses configured hooks/MCP/plugins | `--safe-mode --tools Read,Glob,Grep --permission-mode dontAsk --no-session-persistence` | `--output-format stream-json --verbose`; sanitized system/assistant/result events | Replaces counters from result `usage`; optional input/output/cache/total aliases |
| Codex | Same as ordinary command: `exec --sandbox read-only --ephemeral --skip-git-repo-check --color never`, stdin `-`; repository exploration remains possible | Same command; filesystem sandbox, not a read-tool whitelist | `--json` before trailing `-`; sanitized turn/item events | Replaces counters from `turn.completed.usage`, including cached input when reported |
| Opencode | `run --pure` plus `OPENCODE_PERMISSION={"*":"deny"}` | Same pure mode with wildcard deny and read/glob/grep/list allow | `--format json`; incremental text and step events | Accumulates recognized fields from `step_finish.part.tokens` |
| Pi | `-p --no-session --no-tools --no-extensions --no-skills --no-prompt-templates --no-context-files --no-approve` | Replace no-tools with `--tools read,grep,find,ls`; retain resource-disabling flags | `--mode json`; message/tool/retry/compaction events | Accumulates recognized fields from assistant `message_end.usage` |

`run_with_progress` currently uses the ordinary command for **every** harness.
Claude, Opencode, and Pi therefore lose the above restricted/read-only settings
on that path. Codex retains its ordinary sandbox. A caller must not infer that
progress can already be combined with a chosen restriction.

| Control | Claude | Codex | Opencode | Pi |
| --- | --- | --- | --- | --- |
| Availability checks | Launcher availability plus progress-format probe; Expert Assist must also probe safety flags | Checks required `exec` flags including sandbox, ephemeral, JSON | Version plus progress-format probe; pure/permission behavior needs Expert Assist validation | General availability/progress checks; interview selection separately checks complete restricted/read-only flags |
| Caller cancellation | No handle from blocking runner | No handle from blocking runner | No handle from blocking runner | No handle from blocking runner |
| Runtime/output/input limits | No runner deadline, byte cap, or token cap | Same | Same | Same |
| Monetary/round budget | No runner spending ceiling or round limit | Same | Same | Same; internal retry/compaction events are not AMF rounds |
| Resolved model and billable breakdown | Not returned by common usage contract | Same | Same | Same |

`HeadlessUsage` has four optional counters and no raw usage provenance, cache
write category, reasoning inclusion rule, actual model, price version, or
completeness flag. Shared parsing recognizes only selected flat field names.
Missing nested fields remain unavailable. Accumulated progress is already a
cumulative snapshot; summing every callback would double count. A missing field
in a later step can leave an earlier partial sum looking complete. A single
final-message string also does not prove successful provider completion:
`run_jsonl_command` currently returns a captured message before checking
`event_error` on a successful process exit. Expert Assist needs a stricter
completion contract.

### Effective access is not uniform

Codex's command does not prohibit shell invocation or remove configured MCP
and other external tools. The `run_investigation` comment's broad “cannot run
shell commands” claim is not established for Codex by the command builder or
its predicate (which checks only the sandbox value). Official documentation
describes read-only execution, command and MCP events, and separate config
controls. See [Codex non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode).

Consequently, Codex is **not eligible for a strict packet-only profile with
the current adapter**. Label it “repository reads enabled” and do not claim a
tool whitelist or repository-only filesystem visibility. Establish external
tool/config isolation and no escalation in a controlled runtime test before
enabling its targeted-read pilot. Do not silently substitute it when a
packet-only profile is requested.

For the other adapters, command whitelists/permission overrides express a
stronger tool boundary, but they still need version-specific conformance
checks. No-tools does not prove the harness loads no instructions or has zero
hidden context. Read tools are not proof of a path allowlist. A profile that
requires only a particular repository to be visible needs filesystem isolation
or a mediated reader; merely changing the working directory is insufficient.

## 3. Consultation schema and lifecycle

Proposed versioned records below are logical schemas, not existing Rust types.
Persist immutable request/round revisions; keep mutable lifecycle fields behind
compare-and-swap updates using `revision`. IDs are UUIDs, timestamps are UTC,
hashes are SHA-256 of exact bytes, and unknown values are nullable with a reason.

```text
Consultation {
  schema_version, id, revision, created_at, updated_at,
  origin: {
    project_id, feature_id, feature_session_id, harness,
    canonical_workdir, repository_identity,
    launch_generation, provider_session_id?,
    tmux_server_identity, tmux_session_id, tmux_window_id, tmux_pane_id
  },
  request_revisions: [Request], execution_state, handoff_state,
  active_attempt_id?, owner_instance_id?, owner_heartbeat_at?,
  cancellation_reason?, last_error?, retention_expires_at?
}
Request {
  revision, question, acceptance_criteria: [string],
  attempted_fixes: [{description, outcome, evidence_ids: [id]}],
  evidence: [EvidenceItem], snapshot: SourceSnapshot,
  profile: ExecutionProfile, prompt_id, template_source, template_hash,
  rendered_prompt_hash, rendered_prompt_artifact_id,
  authorized_rounds, parent_result_id?
}
RoundAttempt {
  id, consultation_id, request_revision, round_number, attempt_number,
  owner_instance_id, process_identity?, started_at?, finished_at?,
  terminal_status?, terminal_reason?, result_id?, usage_segments: [UsageSegment]
}
ExpertResult {
  schema_version, consultation_id, attempt_id, request_revision,
  kind: advice | proposed_patch | needs_evidence,
  summary, evidence_ids: [id], assumptions: [string],
  recommendations: [string], validation_steps: [string],
  patch?: {format: unified_diff, artifact_id, sha256,
           base_snapshot_id, files: [{path, before_hash?, after_hash?}]},
  missing_evidence: [{evidence_id?, path?, range?, question?, reason}],
  limitations: [string]
}
Handoff {
  id, consultation_id, result_id, revision,
  original_body_artifact_id, edited_body_artifact_id?, body_hash,
  freshness: current | stale | unknown,
  target_status: ready | busy | missing | changed | unknown,
  delivery_attempt_id?, delivery_stage?, submitted_at?,
  acknowledged_at?, dismissed_at?
}
```

`origin` IDs are authoritative; names are display metadata only. A provider
session ID is optional because not all harness associations are reliable.
Capture the tmux server lifetime and AMF launch generation to distinguish
recreated panes and harness restarts even when window names or pane IDs repeat.
These fields require implementation; current session IDs alone cannot do this.

Validate output against a closed versioned schema with size bounds. Require a
nonempty summary and actionable advice, a parseable patch with base metadata,
or at least one precise missing-evidence request, according to `kind`. Reject
mixed incompatible variants, unknown evidence IDs, absolute/traversing patch
paths, unsupported binary patches, and edits outside the agreed repository.
A proposed patch is text only: parsing or dry-run applicability checking never
applies it. Expert claims of validation remain unverified claims; the
implementer records actual command results separately.

Provider failure, timeout, cancellation, malformed/empty/truncated output,
missing completion signals, and a final text followed by an error cannot
produce a ready handoff. Keep bounded diagnostic output for inspection, marked
incomplete. No automatic paid “JSON repair” call. A user may copy incomplete
advice manually, but AMF must not promote it to a successful structured result.

### Execution transitions

| From | Event | To / required effect |
| --- | --- | --- |
| Draft | User requests consultation | Precall; freeze evidence/profile revision |
| Precall | Continue with valid revision and available profile | Queued; persist authorization before admission |
| Precall | Cancel | Draft; no child and no charge from this action |
| Queued | Acquire ownership and pass admission/freshness checks | Running; persist attempt before spawn |
| Queued | Cancel | Cancelled; never spawn |
| Running | Valid advice/patch and successful terminal completion | Completed; stage Ready handoff, never send |
| Running | Valid missing-evidence result and successful completion | NeedsEvidence; display requested sources and round usage |
| NeedsEvidence | User supplies/selects evidence and requests another round | New request revision → Precall; preserve earlier rounds |
| Running | Provider/spawn error | Failed; retain known usage and actionable error |
| Running | Invalid or incomplete response / enforced output cutoff | Incomplete; no handoff |
| Running | User cancel or deadline | Cancelling; request process-tree termination |
| Cancelling | Tree reaped | Cancelled (or TimedOut for a deadline); retain partial/unknown usage |
| Failed / Incomplete / Cancelled / TimedOut / Interrupted | User retries | New attempt through Precall; never overwrite failed cost |
| Queued / Running / Cancelling | Owner died | Interrupted after ownership reconciliation; no automatic relaunch |

The winning persisted transition decides a cancel/completion race. Once
Cancelling wins, late success is diagnostic only. Closing an overlay parks the
consultation and allows work to finish; **Cancel** is explicit. Shutdown and
feature stop cancel owned active jobs. Source changes set freshness flags,
independently of execution status.

### Handoff transitions and recovery

Handoff state is separate: `None → Ready ↔ Editing → Sending → Submitted`,
with `Ready/Editing → Dismissed` and `Sending → DeliveryUnknown` on an ambiguous
transport outcome or crash. Failed delivery before any input was written can
return to Ready. `Submitted` means AMF submitted to the target input, not that
the implementer accepted, applied, or validated the suggestion. Optional
provider acknowledgment can record receipt separately.

Reopening an overlay or restarting AMF restores Ready/Editing content without
delivery. Dismissed results can be viewed in history but require a new explicit
stage/send action. Submitted results cannot be resent by repeated keypresses.
DeliveryUnknown requires target inspection and explicit resolution; never
retry automatically. Exactly-once agent consumption cannot be promised over
tmux without a receiving harness acknowledgment/deduplication protocol.

Because the database is global, startup must not interrupt jobs owned by a
different live AMF instance. Use an owner UUID, heartbeat, boot/process start
identity, and conditional ownership claim. Reconcile an expired owner before
marking Interrupted; do not kill an arbitrary reused PID. A supervisor/watchdog
must terminate verified orphaned process groups after parent death. Persisted
PID alone and `Drop` cleanup do not survive a crash.

## 4. Evidence collection and freshness

```text
EvidenceItem {
  id, kind: source | diff | failure | session_excerpt | user_note,
  source_ref: {relative_path?, session_id?, provider_session_id?,
               artifact_id?, byte_range?, line_range?},
  captured_at, source_hash, excerpt_hash, original_bytes, included_bytes,
  included_ranges, omitted_ranges, omission_reason?, retrievable,
  text_or_artifact_id
}
SourceSnapshot {
  id, repository_identity, canonical_workdir, head_oid?,
  index_fingerprint, worktree_manifest_hash,
  files: [{path, kind, content_hash?, mode, symlink_target?, absent}],
  scope, capture_started_at, capture_finished_at, stable
}
```

Collect locally without a model: question and acceptance criteria first,
attempted fixes next, then selected diagnostics, changed hunks, referenced
symbols with bounded surrounding lines, and explicitly selected session
excerpts. Preserve command, working directory, exit status, timestamp, and
snapshot association for failure evidence. Do not run arbitrary validation
commands merely to construct a packet. Let users attach existing results.

Deduplicate identical content while retaining every source reference. Reserve
space for the question, acceptance criteria, source manifest, and omission
markers before selecting excerpts. Rank remaining material by explicit user
selection, failure location, changed hunk, and referenced dependency, breaking
ties by normalized path and range. If mandatory material exceeds the cap,
require narrowing the request instead of silently dropping it. Unparseable
logs get labeled head/tail excerpts; binary or excluded sources get a reason.
Do not drop counterevidence merely because it is not a failure line.

Resolve transcript evidence through the originating AMF session and verified
provider source binding. Never use “latest transcript in this workdir.” If
identity cannot be established, offer selected pane text with its weaker
provenance or user-entered notes. Do not capture another session or forward an
entire transcript. User selection and automated exclusion rules should avoid
credentials and unrelated files; neither is a guarantee of secret detection.
Treat repository text and logs as evidence, not instructions authorizing tools,
paid follow-ups, delivery, or permission changes.

Omissions carry exact ranges/byte counts, a stable evidence ID, and a retrieval
route. A packet-only expert cannot open an artifact path. It returns
`needs_evidence` naming the ID/range; AMF retrieves a bounded excerpt, lets the
user adjust it, and opens a new pre-call notice. A targeted-read expert can
read permitted files directly; captured logs outside that environment still
need an explicitly exposed artifact or the same request mechanism. User notes
can be answered in the form. Expired, unavailable, or excluded evidence must
say `retrievable=false` with a reason; an unusable pathname is not retrieval.

Hash bytes, not just mtimes or HEAD. Include dirty tracked files, index state,
selected untracked files, deletions, file modes, and symlink targets. Reject
path traversal and symlink escapes during collection. For packet mode, the
freshness scope includes every source behind an excerpt and every proposed
patch base. Record the broader HEAD/index context too; branch movement merits
reassessment even if those excerpts match. Unchanged hashes mean “selected
evidence current,” not “the whole repository is proven equivalent.”

For targeted exploration, prefer a private immutable snapshot of the allowed
repository contents, including relevant uncommitted changes. Capture a manifest
before/after copying and reject/retry locally if bytes changed during capture.
Exclude `.git` credentials, unrelated worktrees, build outputs, and unselected
external paths; list exclusions. Read-tool configuration alone does not enforce
this visibility boundary. If a live worktree is used experimentally, fingerprint
the full declared readable source scope before and after; unknown read sets or
mid-run changes yield unknown/stale results. Document this weaker trial arm.

Check fingerprints before launch, on result arrival, on reopening a ready
handoff, and immediately before Send. A changed patch base disables patch Send;
offer refreshed evidence and a new consultation, or explicitly create an
advice-only handoff that omits the patch and states what changed. Unknown
freshness also blocks patch delivery. For advice, allow an explicit “Send with
stale-evidence note” action. An atomic repository snapshot at delivery is not
available through tmux; include base hashes and instruct the implementer to
recheck immediately before applying and then validate. A successful dry-run
patch check alone does not establish semantic freshness.

## 5. Execution profiles and limits

Keep the current implementer harness/model unchanged for the first pilot.
Require a named, explicit expert profile. Recommend Claude's no-tools adapter
as the first packet prototype because the configured tool boundary is simple;
compare its read-tool profile using the **same expert model** to isolate the
effect of exploration. This is an adapter-development priority, not a claim
that Claude is cheapest or highest quality. Pi and Opencode can enter after
their conformance/usage tests. Codex is a separate targeted-read candidate after
its access gaps are resolved. No concrete model default is selected here.

```text
ExecutionProfile {
  id, revision, harness, requested_model, resolved_model?, cli_version,
  requested_access: packet_only | targeted_read,
  effective_access: {tools, filesystem_scope, external_tools, config_policy},
  effort?: {value, support: enforced | advisory | unsupported},
  limits: {max_rounds, max_attempts, packet_bytes, estimated_input_tokens?,
           response_bytes, output_token_target?, elapsed_seconds,
           spend_notice_amount?, currency?},
  billing_basis: metered_api | subscription_marginal | modeled_api_equivalent,
  price_snapshot_id?
}
```

Reusable defaults belong in a dedicated Expert Assist configuration section,
resolved global → project → per-consultation override, then frozen in the
request. Implementation must follow existing config resolution conventions.
Keep it separate from `FeaturePreset` until feature-creation integration is
decided. Unsupported required restrictions fail before spawning; model or
harness fallback requires a changed profile and a fresh notice. Never silently
broaden access to obtain an answer.

| Approach | Expected benefit (hypothesis) | Cost/quality risk | Initial use |
| --- | --- | --- | --- |
| Compact packet, no tools | Predictable supplied context and reproducible evidence | Missing dependencies may cause extra paid rounds or wrong advice | Narrow failure with known source locations |
| Packet plus targeted reads | Expert can check dependencies and reduce omissions | More exploration/reasoning, hidden context, less predictable usage | Cross-file ambiguity after a focused packet cannot answer |
| Codex read-only sandbox | Repository exploration through its existing execution path | Broader tool/config surface than a read whitelist; cannot serve as strict packet-only comparator | Separately labeled conformance-tested trial |

Recommend **one paid round per user action** for the pilot. Every evidence
follow-up or retry passes through pre-call again. An AMF round is one expert
invocation, not a provider turn, internal retry, or compaction. Preserve those
internal events/usage separately. A future user-selected multi-round allowance
is deferred; it must not imply automatic handoff delivery.

Suggested pilot starting values: two total rounds, three total attempts
(including failures/retries), 48 KiB rendered packet, 32 KiB response, and
180 seconds per attempt. Reserve about 8 KiB of the packet for mandatory
question/criteria/manifest data. Exceeding a consultation ceiling requires an
explicit recorded override and new notice, not an invisible reset. These are
engineering starting points to test, not settled budgets or a model-window
claim. Set no default dollar ceiling until the billing basis is selected.

| Limit | Current status | Prototype enforcement / honest UI label |
| --- | --- | --- |
| AMF rounds and attempts | No consultation scheduler | Hard launch counter; count failed attempts; no automatic repair call |
| Rendered packet bytes | Not bounded by runner | Hard pre-spawn byte cap after template resolution |
| Context tokens | No common tokenizer or complete hidden-context accounting | Estimate only; byte cap is not a total context cap |
| Response bytes | Runner buffers without a cap | Hard bounded reader/storage; terminate on overflow and mark Incomplete; separately bound JSONL lines, total stream, and stderr |
| Generated/reasoning tokens | No common runner control | Prompt target only unless a version-tested harness control is added; truncating visible text does not cap billed reasoning |
| Elapsed time | No blocking-runner deadline | Supervisor deadline and bounded termination grace; charges may settle later |
| Spending | Optional usage too incomplete for ceiling | Estimate/notice only; never display a guaranteed cap; provider billing limits require separate verification |
| Repository/tool restrictions | Harness-specific builders | Enforce only conformance-tested profile; prompt wording is advisory |

Also bound source collection and queue waiting independently of run time, and
count both in elapsed reporting. Admission should respect the existing agent
limit, with one active consultation per origin session initially. Do not treat
`HeadlessLease` as a semaphore or global cross-process counter.

## 6. Session flow and explicit delivery

Add **Ask expert** to the embedded agent session's leader actions, following
[`handlers/view.rs`](../../src/handlers/view.rs) and
[`ui/pane.rs`](../../src/ui/pane.rs). Choose the exact key during implementation
after checking menu/help collisions; `a` already opens local actions and `E`
opens prompt overrides. A local-action picker entry can expose the action
without claiming either key. Disable it on terminal/editor/non-agent sessions.
Reinvocation opens the origin's active consultation or ready card.

1. Capture the origin IDs before opening the form. Display the target session,
   question, criteria, attempted fixes, selected evidence/omissions, expert
   profile, effective access, and hard versus advisory limits. Filling the form
   is local. Preserve any composer draft when entering through its leader menu.
2. **Consult** validates the form and resolves `expert_assist.consult`. Extend
   `PrecallAction` and notice metadata with consultation ID, request revision,
   effective profile, model, and evidence digest. Keep Continue, View, Edit,
   Cancel. Continue does not require viewing. Template editing re-resolves and
   revalidates the rendered prompt; changed evidence/profile requires an updated
   notice, not reuse of clearance for another consultation.
3. Show queued/running activity and elapsed time without blocking session use.
   Persist progress/status outside `AppMode` so switching overlays cannot drop
   the job. Closing the overlay parks it; Cancel terminates it. A ready toast
   links back to the consultation without switching target or stealing input.
4. Missing evidence gets selectable source requests and **Consult again**.
   Failure/cancellation shows retained evidence, usage completeness, and an
   explicit retry action. Every new paid attempt uses pre-call.
5. The ready card shows a short result summary, target, profile, freshness,
   rounds and available usage. Offer **Send**, **View/edit**, and **Dismiss**.
   Send delivers the staged text directly when the checks below pass; opening
   an editor is never a prerequisite.

### Composer and target handling

Use a consultation-specific draft keyed by `(consultation_id, handoff_revision)`.
The editor can reuse composer rendering/editing, but needs a handoff context
with immutable origin IDs and a dedicated submit callback. Do not call
`open_compose_seeded` unchanged: it replaces ordinary saved draft text, keeps
possibly unrelated images, and resolves through the current view's names.
Do not inherit ordinary attachments. Closing View/edit saves the consultation
draft and restores the previous view/draft. Explicitly persist the edited body
and hash; Send uses that revision rather than regenerating the original seed.

If an ordinary AMF draft exists, preserve it separately and disclose “Your
existing draft is saved.” Sending the expert handoff must not clear or append
that draft. Restore it afterward. If text is already in the harness's own input
buffer, do not clear it with `C-u`, paste over it, or send Enter into it. Existing
[`submit_compose`](../../src/app/compose.rs) behavior therefore needs a dedicated
Expert Assist transport path, not a direct call through generic submission.

| Target condition at Send | Behavior |
| --- | --- |
| Same origin/generation, live agent, idle with empty input | Perform freshness and revision checks, then submit exactly this handoff |
| Busy or permission prompt | Keep Ready; offer Open target. When it becomes idle, notify only; a new Send action is required |
| Readiness/input unknown | Keep Ready and open target on request; require the user to establish empty, ready input before another Send, or use a verified harness readiness signal |
| Session/feature deleted, stopped, or pane missing | Keep result available as orphaned history; disable Send; never choose the currently selected session or create a replacement automatically |
| Same display name but new session, process generation, or provider conversation | Mark target changed; require explicit origin rebind and recheck evidence through a new staged revision |
| Changed source evidence | Apply section 4 rules; do not silently forward a stale patch |

Thinking/status indicators in [`sync.rs`](../../src/app/sync.rs) are useful busy
hints, not proof of empty input or atomic readiness. Serialize AMF input during
submission and revalidate the pane/process identity immediately before writing.
Other tmux clients can still race input: document this transport limitation,
and gate pilot Send on a supported harness/target condition. A production
guarantee against those races needs a harness input protocol or equivalent
coordination; idle detection alone is insufficient.

Persist a delivery attempt with a unique handoff revision before transport.
Use a unique named tmux paste buffer, a stable pane ID, bracketed paste, and a
separate submit step; clean the buffer afterward. Current
[`tmux.rs`](../../src/tmux.rs) paste uses an unnamed buffer, and literal sends
can fall back after a control-client error. Those paths must not cause an
ambiguous handoff to be sent twice. Track prepared/pasted/submitted stages and
never retry a possibly executed input operation automatically. A crash after
Enter but before the database commit becomes DeliveryUnknown. A durable CAS
claim prevents simultaneous AMF instances or repeated Enter from submitting
the same revision. It cannot prove the receiving agent consumed it once.

The handoff contains consultation ID, question/criteria, advice, relevant source
references and base hashes, omissions/limitations, and proposed validation.
Include the bounded patch inline when it fits so the implementer need not read
the global AMF database. For larger artifacts, require a verified accessible
export into a disclosed local artifact location or a smaller handoff; do not
send an inaccessible DB/blob reference. Applying remains the implementer's job.

## 7. Minimal prototype and persistence

Build an opt-in vertical slice using an isolated database/repository and fake
harness first, followed by one conformance-tested real harness. The prototype
accepts a manual question and selected source/failure excerpts, runs one
restricted round with progress, persists a validated answer or missing-evidence
request, and stages an optional-edit handoff to the origin. It must exercise
cancel and recovery. No automatic model selection, summarization, expert edits,
feature-creation integration, or usage dashboard redesign is needed.

| Area | Proposed implementation seam | Exit requirement |
| --- | --- | --- |
| Orchestration | New `src/app/expert_assist.rs`, domain state, `App` job registry keyed by consultation/attempt ID | UI modes only observe/control jobs; session switching does not rebind them |
| Runner | New explicit `HeadlessExecutionPolicy` and `start_with_policy_and_progress(...) -> HeadlessJobHandle` in `headless.rs` | Build restricted/read-only commands first, then JSONL args; preserve existing callers' semantics |
| Process owner | Handle with progress/result receiver, cancel signal, and child supervisor | Lease held until tree reaped; bounded streams, deadline, panic/error cleanup, parent-death handling |
| Prompt | `PromptId::ExpertAssistConsult`, stable key `expert_assist.consult`, template in `src/prompts/defaults.rs` | Register `ALL`, spec, placeholders, override resolution and pre-call redispatch |
| Storage | New `src/db/expert_assist.rs` and additive migration | Atomic request/round/result/delivery revisions and owner claims; no runtime DB edits in this investigation |
| Rendering/input | New expert dialog and handler, dashboard dispatch and leader/local action registration | Form, progress, evidence-needed, ready/edit/failure states with optional viewing |
| Handoff | Target-aware composer context plus dedicated tmux submit operation | Preserve ordinary drafts and attachments; no auto-send, fallback retry, or wrong-target input |
| Attribution | Consultation-local usage segments and evaluation export | Known, partial and unknown cost remain distinguishable |

The new runner's terminal result must include exit status, completion/error
classification, bounded final response, and final usage completeness. Errors
take precedence over captured text. Keep progress generic (“Examining
evidence”), since existing activity strings mention PR reviews. Preserve only
sanitized activity in UI/debug logs; store usage-only raw fields separately,
not full reasoning, commands, or provider event payloads. Validate restrictions
in release builds rather than relying on `debug_assert`.

Proposed placeholders: `question`, `acceptance_criteria`, `attempted_fixes`,
`evidence_packet`, `source_manifest`, `prior_round_summary`, `effective_access`,
`response_contract`, and `limits`. Prior-round context is deterministic selected
question/result text plus evidence references, not paid summarization. Templates
can change instructions but cannot alter runner access, limits, output schema,
or handoff authorization. Validate required fields and rendered size after
overrides; do not assume the generic template resolver enforces them.

### Migration and cleanup requirements

Use consultation-owned tables for consultations, immutable requests/attempts,
artifacts, usage segments, and handoffs/delivery attempts. Index origin IDs,
execution state/owner, and expiry. Use foreign keys within these tables and
unique constraints for `(consultation_id, round_number, attempt_number)` and
handoff delivery claims. Include optimistic revision checks for multiple AMF
instances. Bound content in application validation; store schema versions with
serialized payloads so unknown versions fail safely.

**Do not add cascading foreign keys from consultations to `projects`,
`features`, or `feature_sessions`.** [`db/store.rs`](../../src/db/store.rs)
currently saves via full replacement: it deletes projects and cascades through
features/sessions before reinserting them. Such a foreign key would erase
consultations during an ordinary save. Keep stable origin IDs as logical
references, as existing independent domain histories do, and explicitly
reconcile real deletion after a completed store transaction. A future store
upsert redesign is separate work, not required to store consultations safely.

Choose the next migration number at implementation time. Test upgrade from the
previous schema and repeated initialization, rollback on migration error, and
ordinary store saves preserving consultations. Existing installations start
with no consultation rows; no transcript backfill or rate-table rewrite.
Older binaries must not accidentally discard the new domain tables during
store saves. Unknown consultation schema versions should be viewable as
unsupported metadata, never runnable/deliverable by guessing defaults.

For the pilot, recommend deleting raw evidence and patch artifacts 7 days after
a terminal consultation, with an explicit keep/export option. Retain minimal
usage/outcome metadata for the evaluation's stated retention period; users can
delete it too. Unsatisfied Ready/Editing handoffs have no automatic expiry by
default; surface their age and provide explicit deletion. These are proposed
retention choices to confirm before rollout, not existing AMF policy.
Expired artifacts leave non-retrievable tombstones so old references are honest.

On real feature/project deletion, cancel active jobs, prevent delivery, and
remove sensitive artifacts under the retention/deletion policy; preserve only
permitted orphan metadata. Session removal marks its targets missing without
immediately erasing useful results. Clean temporary snapshots, named paste
buffers, incomplete artifacts and verified orphan jobs on shutdown/startup.
Only reclaim resources with recorded AMF ownership. Never alter global Claude
or Opencode configuration; prefer per-process flags/environment. Persistence
failure before spawn blocks launch; after execution, retain the result in
memory with an error and disable Send until its delivery record is durable.

### Narrow usage attribution

```text
UsageSegment {
  id, consultation_id?, attempt_id?, evaluation_run_id?,
  phase: evidence | implementer | expert | retry | handoff | validation,
  harness, cli_version, requested_model, actual_model?, provider_request_id?,
  start_cursor?, end_cursor?, raw_usage_fields, parser_version,
  normalized: {uncached_input?, cache_write_input?, cache_read_input?,
               output_including_reasoning?, total_logical_tokens?},
  completeness: complete | partial | unknown, missing_reason?,
  billing_basis, currency, price_snapshot_id?, computed_cost?, cost_status
}
```

Preserve provider usage objects only, including unknown category names, and
whether events are deltas or cumulative snapshots. Deduplicate by event/request
identity or stream sequence; do not sum cumulative progress snapshots. Track
missing step usage explicitly. Model identity from a requested alias is not
proof of the resolved model. Pricing needs a pinned per-model rate snapshot,
currency, billing basis, category inclusion rules, thresholds and applicable
fees. Cache categories must be disjoint; reasoning already included in output
must not be added again. Unknown usage/cost is never zero.

For implementer cost, use verified session-source boundaries before and after
each evaluation run; include pre-consultation work. If this cannot isolate the
run, use dedicated trial sessions and a provider usage export, or mark it
unpriced. Do not attribute an expert's usage to the interactive session's
ordinary cache or price both with `TokenPricingConfig`. Provider internal
retries/compaction may be incompletely reported; reconcile against available
billing records and mark unresolved amounts partial. Cancellation bounds local
process life, not necessarily remote billing. The trial exporter is sufficient;
a general session accounting migration is out of scope.

## 8. Evaluation protocol

**Status: protocol defined; no experiments run and no measured savings.** The
reference document's prices and hypothetical percentages are not inputs to a
claim of feasibility at comparable quality. Recheck model availability and
prices at execution time; record exact versions and billing basis before runs.

### Matched trials

Prepare immutable task bundles: repository and dirty-state snapshot, task
description, initial failing behavior, available diagnostics, dependency/tool
versions, acceptance criteria, and fixed validation commands. Include narrow
bugs, cross-file defects, ambiguous failures, and changes with subtle regression
risk. Hide reference solutions from all agents. Use fresh sessions and isolated
copies of the **same** starting bundle for each arm; no cross-arm transcripts,
patches, or learned hints. Record cache state/order and randomize order to reduce
warm-cache or operator effects.

| Arm | Execution | Purpose |
| --- | --- | --- |
| A: expert-only | Configured expert harness/model performs the complete change with the same allowed implementation tools and validation as the implementer in B/C | Baseline accepted-change cost; not a restricted consultation pretending to be implementation |
| B: implementer + packet expert | Existing implementer works, user requests consultation at the registered trigger, bounded packet expert advises, user sends, implementer applies and validates | Complete assistance workflow with no-tools expert |
| C: implementer + targeted-read expert | Same implementer, trigger, expert model and starting packet as B, plus the declared read environment | Measures whether exploration reduces omissions/retries enough to pay for itself |

Keep B/C's expert model, effort where supported, and budgets equal. Use the same
expert model in A, documenting its implementation permissions separately.
Codex packet-versus-read cannot be a matched access ablation with the current
adapter; run it as a separate labeled harness/profile comparison or defer it.
An optional implementer-only arm helps identify tasks that did not need expert
help at all. Do not conclude “lowest cost” among all workflows if that comparator
or relevant pairings were not evaluated.

For controlled trials, register a trigger such as the first failed fix/validation
cycle, or a predefined design-block question, before seeing outputs. A human
initiates the actual consultation. Count all implementer effort leading to the
trigger; do not restart cost accounting there. Also report a separate natural
user-choice cohort later, because a fixed trigger may over-consult simple tasks.
Operators follow the same assistance rules in each arm and log manual guidance.

Recommend a diagnostic pilot of 12 tasks across those categories, two repeats
per arm (72 runs for A/B/C). This is a workflow/variance pilot, not adequate by
itself to establish quality equivalence. Before running it, register task list,
profiles, trial ceilings, stopping rules, review rubric and billing basis.
Use pilot variance to size a separate held-out comparison; freeze its sample
size and non-inferiority tolerance before examining held-out outcomes. No
optional stopping when savings first look favorable.

### Full workflow accounting

Begin before evidence preparation and end at acceptance or a recorded terminal
failure. Record deterministic collection, snapshot creation, queueing, every
expert round, all implementation attempts, provider and AMF retries, failed and
cancelled work, handoff preparation/editing/submission, rework, and validation.
Include background model calls attributable to the workflow. Record human
preparation/review time and local machine time even when their monetary value
is excluded; apply the same exclusion in all arms. If assigning labor or compute
prices, present that as a separate cost basis with declared rates.

Select one primary billing basis before the trial:

- Metered API: actual provider charges where attributable, otherwise a labeled
  calculation from complete usage and frozen rates.
- Subscription marginal charges: actual attributable overage/incremental
  charges; included tokens are not automatically API-priced spending. Record
  allowance consumption separately and do not treat a zero marginal bill as
  unlimited free capacity.
- Modeled API equivalent: a normalized comparison, explicitly not the user's
  actual subscription bill. Report separately from measured monetary charges.

For disjoint billable categories, apply each model's rates to each segment,
including cache-write/read and generated output including reasoning, then add
tool/service fees. Do not mix billing bases or currencies within an aggregate.
Report complete, partial, and unknown runs separately. Known subtotals can be
lower bounds, never complete costs. Do not drop unpriced failed runs to make
assistance look cheaper; resolve them or withhold the primary cost conclusion.

```text
cost_per_accepted_change(arm) = sum(cost of ALL assigned runs in arm)
                                / count(accepted changes in arm)
saving = 1 - cost_per_accepted_change(assisted)
             / cost_per_accepted_change(expert_only)
```

If there are no accepted changes, accepted-change cost is undefined (report the
failures and total spend). If baseline cost is zero, percentage saving is
undefined. Include paired task differences, uncertainty intervals clustered by
task, completion rate, tail costs and failed-run cost. More tokens with lower
accepted-change cost can be a success; fewer premium tokens alone cannot.

### Quality and reporting

Acceptance requires identical functional criteria, focused regression tests,
and independent review blinded to arm when practical. Review correctness,
maintainability, unnecessary scope, security/reliability regressions, and
compatibility. Record severity and remediation cost for missed defects,
unsupported assumptions, omitted evidence, stale advice, and incorrect patch
application. Test passage alone does not establish comparable quality.

For the confirmatory run, a reviewer-approved non-inferiority margin must cover
acceptance rate and defect severity; require no unresolved critical defects.
Numerical tolerance and sample size remain evaluation setup blockers. Do not
choose a cheaper default when the quality interval cannot exclude unacceptable
regression. Re-review after a fixed follow-up window chosen before trials to
capture late defects; include their rework in an updated cost result.

Export one row per attempt/phase plus a task/arm summary with:
snapshot ID, criterion/rubric version, harness/model/profile, trigger,
evidence/omissions, rounds/retries, usage categories and completeness, billing
basis and rates, cost, acceptance/rejection reasons, defect findings, wall time,
active execution time, queue time, and human time. Summaries compare total and
premium tokens separately, elapsed time, quality, and accepted-change cost.
Select only the cheapest **evaluated** profile meeting the quality bar, or report
that no tested configuration qualifies. Preserve negative and inconclusive
results.

## 9. Verification cases

These are requirements for later implementation, not tests run by this design
investigation. Use fake CLI scripts, temporary repositories/databases and
isolated tmux servers for deterministic Rust tests. Run version-specific harness
conformance experiments separately; command-shape assertions alone do not
prove effective restrictions.

| Case | Required assertion |
| --- | --- |
| Restricted execution with progress | Each harness keeps its exact policy flags/env when JSONL/model flags are assembled; ordinary-policy fallback is rejected |
| Hostile project config | Hook/plugin/MCP/tool allowances cannot grant writes or extra tools under a claimed profile; test configured external tools and Codex separately |
| Packet-only Codex request | Fails eligibility before spawn rather than being labeled no-tools |
| Filesystem scope | Path traversal, external symlinks and excluded artifacts cannot bypass the declared snapshot/read boundary |
| Unsupported CLI/model/effort | Preflight gives an actionable unsupported-profile result; no implicit fallback or stronger permissions |
| Omitted evidence | Marker survives compaction/capping; ID/range retrieves the correct source or an explicit unavailable result; no whole transcript sent |
| Multiple sessions in one workdir | Origin-bound excerpt lookup never chooses the latest neighboring transcript |
| Source changes | Dirty file, index, untracked source, symlink, deletion and HEAD changes detected at each checkpoint; mid-capture changes rejected |
| Stale patch | Changed/unknown base blocks patch Send; advice-only revision visibly drops the patch; implementer still rechecks before application |
| Bad/incomplete output | Empty/truncated/malformed JSON, invalid patch paths, missing completion, and text followed by error never stage Ready |
| Cancellation/deadline | Child and descendants terminate, lease stays held until reaped, UI remains responsive; late completion cannot resurrect cancelled output |
| Stream overflow/stall | Bounded line/stream/stderr readers stop runaway output; blocked pipes/writer and silent child obey deadline |
| Restart/multiple instances | Live foreign owner is preserved; crashed owner reconciled without PID-reuse kills; Ready/Editing restored unsent; no automatic rerun |
| Missing/changed target | Deleted/stopped/recreated session, renamed window, provider restart and tmux server restart cannot send to a different agent |
| Busy/unknown input | No interrupt, `C-u`, paste, or Enter; becoming idle never triggers an automatic send |
| Existing drafts | Ordinary text/images survive consultation editing and sending; edited handoff is what is delivered; cancelling editor preserves it |
| Optional editing | Ready → Send works without opening preview/editor; View/edit and Dismiss cause no transport calls |
| Duplicate/ambiguous sends | Repeated keys and concurrent AMF instances produce one claimed attempt; crash after paste/Enter enters DeliveryUnknown without transport fallback retry |
| Pre-call binding | Cancel spawns nothing; template edits update prompt; stale clearance cannot launch another consultation/revision/profile |
| Usage completeness | Missing counters remain unknown; cumulative snapshots not summed; partial step usage marked partial; cache/reasoning inclusion avoids double counting |
| Pricing attribution | Each actual model/billing basis uses its own snapshot; missing actual model/rates suppress a complete cost claim; failures remain counted |
| Migration/store save | Upgrade/reopen works; ordinary full-replace project saves preserve consultations; explicit deletion/retention removes only intended artifacts |
| Persistence failure | Pre-spawn failure prevents launch; post-run failure preserves recoverable data and blocks non-durable delivery |

Require focused Rust tests for the runner policy/parser/process owner, domain
transitions, evidence selection/fingerprints, storage migrations/CAS, and handoff
transport/draft handling. Run `cargo check` and `cargo clippy` after the focused
tests for a later implementation. Real-harness restriction checks must record
CLI version, effective configuration, attempted operations and observed results;
any paid tests belong in the evaluation ledger. Screenshots are optional only
when explicitly requested and do not substitute for behavioral checks.

## 10. Feasibility and production backlog

**Recommendation: proceed to an opt-in prototype; do not yet ship automatic
defaults or claim cost savings.** The existing runner, prompt registry, SQLite
domains and composer provide enough structure for user-initiated consultations.
The main work is combining policy with progress/process ownership and making
origin-bound handoffs durable. The economic hypothesis remains untested.

| Remaining blocker | Required evidence/decision | Blocks |
| --- | --- | --- |
| Effective harness restrictions | Real conformance results, especially Codex configuration/external-tool access | Enabling that harness/profile |
| Cancellation and crash ownership | Tree cleanup, watchdog and multi-instance recovery tests | Any production paid job |
| Target readiness and delivery ambiguity | Stable generation binding, input-safe transport, explicit unknown outcome UX; document cross-client limits | Production direct Send |
| Complete cost attribution | Billing basis, actual model/category mapping and failed-run reconciliation | Monetary savings claim |
| Model/policy selection | Matched trials and held-out quality/cost result | Concrete model defaults |
| Budgets and retention | Pilot configuration followed by workload/owner feedback | Broad rollout defaults |
| Comparable quality | Registered tolerance, adequately sized held-out sample, independent review and follow-up | Claim of equivalent quality |

Ordered production implementation backlog:

1. **Runner foundation complete.** Define typed access profiles/capability reports; extend availability checks
   and build restricted/read-only structured commands. Fix terminal error
   precedence and add fake-harness conformance tests.
2. **Owned-job foundation complete.** Cancellable jobs, bounded readers,
   deadlines, handle-drop and parent-death cleanup are implemented. App-level
   admission, shutdown wiring, and recovery invocation remain integration work.
3. **Persistence foundation complete.** Consultation-owned storage/migrations,
   immutable revisions, multi-instance reconciliation, and explicit deletion
   survive full-replace store saves. Feature deletion and retention policy
   wiring remain integration work.
4. **Evidence foundation complete.** Origin identity/generation binding,
   deterministic explicit selection, SHA-256 manifests, omission retrieval, and
   freshness checks are implemented. Session excerpt adapters and persistence
   wiring remain integration work.
5. **Prompt/pre-call foundation complete.** Register the expert prompt and
   freeze consultation provenance in the existing pre-call notice. Add the
   consultation form, progress, missing-evidence, failure and ready states with
   one round per action.
6. **Handoff foundation complete.** Consultation-owned editable drafts, target
   readiness checks, unique delivery claims, and durable unknown-outcome recovery
   are implemented. Composer integration and richer target-generation discovery
   remain follow-up work.
7. **Prototype usage/export complete.** Preserve consultation-local usage and
   export one bounded row per attempt with incomplete cost fields until a price
   snapshot is selected; validate real harness profiles in isolated conformance
   runs.
8. Run the registered pilot, resolve billing/limits/retention, then run held-out
   quality/cost trials. Choose qualifying defaults or record a no-go/inconclusive
   result. Complete focused Rust tests, `cargo check` and `cargo clippy` before
   enabling production support.
9. Consider additional harnesses, feature-creation defaults and multi-round
   allowances only after evidence supports them and separate product decisions.

### Prototype implementation progress

The first item is implemented in [`headless.rs`](../../src/headless.rs) and
[`headless/policy.rs`](../../src/headless/policy.rs). The new
`run_with_policy_and_progress` entry preserves the selected boundary while
adding structured output. `PacketOnly` and `ReadOnlyTools` reject Codex;
`ReadOnlySandbox` reports its broader sandboxed-command access explicitly.
Required policy/model/progress flags are probed on explicit runs. The ordinary
progress wrapper keeps its existing policy and availability behavior.

Independent release-time command validation rejects weakened flags/env.
Provider failures, missing error details, nonzero exits with malformed stdout,
blank answers, and unsuccessful Pi retries cannot be hidden by captured text.
Pi retry recovery requires explicit success and a replacement answer.

The new [`headless/job.rs`](../../src/headless/job.rs) API adds explicit
executable/model selection, bounded preflight and execution, nonblocking pipe
I/O, coalesced progress, cancellation, deadlines, and required terminal events.
A parent-pipe watchdog terminates the owned process group after cancellation
or parent loss; cleanup holds the resource lease and runs off the UI thread.
This boundary does not contain children that deliberately detach into another
process group. Real-harness process behavior still requires conformance trials.
Existing blocking callers retain their earlier lifecycle.

[`db/expert_assist.rs`](../../src/db/expert_assist.rs) and migration 36 add
immutable request/attempt/handoff revisions and conditional ownership updates.
Consultations have no cascading foreign keys to the full-replace project store.
Durable cancellation overrides late completion; failures retain available usage.
Single delivery claims persist ambiguous outcomes without retrying. Recovery
preserves live or unidentifiable owners and marks confirmed abandoned attempts
interrupted without launching work or killing persisted PIDs. Raw successful
responses still require app-level result validation before staging a handoff.

The evidence foundation is implemented in
[`db/expert_assist/evidence.rs`](../../src/db/expert_assist/evidence.rs). It
sorts required selections ahead of optional excerpts, rejects traversal and
symlink escapes, records SHA-256 source/excerpt fingerprints and a scoped
manifest, and emits bounded omission ranges with stable IDs. Retrieval checks
the exact consultation origin and current source hash before returning a range;
changed or unavailable evidence requires a fresh packet. This remains an
explicit-selection API until the consultation form supplies source/session
selectors and persistence records the resulting packet.

The prompt registry now includes `expert_assist.consult` with explicit question,
criteria, attempted-fix, evidence-packet, and access-boundary placeholders.
`PendingPrecall` can carry the consultation ID, request revision, and evidence
digest; the notice renders these values so confirmation remains bound to the
request that was reviewed. The form, worker progress, missing-evidence state,
and ready handoff card are wired into the app flow.

Submitting the form now resolves the layered prompt and enters the existing
pre-call notice with a frozen request revision and evidence digest. The notice
remains an explicit user gate; confirmation claims the durable attempt and
launches the owned worker only after the request is persisted.

With a selected feature/session, submission also persists the consultation
origin and request revision before opening the notice. A missing database or
feature context leaves the draft in place and launches nothing. The initial
profile is explicit Claude packet-only with the prototype limits; model/profile
selection remains part of the remaining form work.

After explicit pre-call confirmation, the app claims the durable attempt before
starting `HeadlessRunner::start_job` on a background thread. That thread opens
its own database handle and finishes the attempt with the stored owner, so
late or mismatched callbacks cannot overwrite it. The app retains and polls the
active handle from the main loop, updates running activity, and maps terminal
outcomes to Ready, MissingEvidence, or Failed states. Explicit Send/edit/dismiss
handoff actions are connected; structured omission retrieval remains a
production follow-up.

Successful responses are staged into consultation-owned handoff revisions.
Ready supports viewing, editing into a new immutable revision, and dismissing.
Send validates the originating target generation and tmux transport before any
transport call; ambiguous failures are persisted without automatic retry.

The evaluation exporter emits one attempt row per consultation with status,
usage counters, completeness, elapsed time, and explicit null cost/billing
fields. Failed and incomplete attempts remain included, preserving negative and
unknown results without implying a billing estimate.

The Send action validates the persisted tmux session and exact window/pane IDs
before claiming a delivery. It sends literal handoff text followed by Enter only
after that check; a transport failure becomes `delivery_unknown` and is never
retried automatically. View/edit and Dismiss remain local actions.

The embedded-session local action picker also exposes `ask-expert` when a
feature or session is selected, preserving the current view while opening the
form. It is intentionally unavailable at the project-only level.

Validation passed: full `cargo test -- --test-threads=2` (2,582 passed, zero
failures), including 60 focused headless tests and 18 Expert Assist DB/evidence
tests, plus `cargo check`, `cargo clippy`, formatting, and whitespace checks. No
real-harness conformance or paid experiment was run. Items 8 and 9 remain
production evaluation and hardening work; usage remains explicitly incomplete.

The design investigation is complete when this document covers the approved
tasks; remaining production backlog items and unrun experiments remain
pending. No measured cost, token reduction, quality equivalence, or saving is
reported by this investigation.
