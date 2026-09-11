# Automatic Expert Consultation: placement investigation

## Conclusion

The highest value insertion point is the existing plan interview's final review
gate, immediately after plan synthesis and before the user accepts the plan or
AMF starts the feature. At that point AMF has a brief, answered discovery
questions, a concrete plan, acceptance criteria, the feature workdir, and a
deterministic repository context. One bounded expert review can catch an
incorrect approach before implementation spends tokens on it. The result can
be attached to the plan review and kickoff prompt; it does not need a separate
session, handoff composer, or automatic message delivery.

This should be an opt-in or policy-controlled preflight at first, with a local
risk and value check deciding whether to spend the expert call. It should not
run on every plan round or every feature start. The review may return a small
set of clarification questions; the user answers them in the same review
surface and AMF permits one bounded follow-up. The closed Expert Assist PR
implemented useful runner, evidence, and persistence foundations, but its
manual question form and post-hoc handoff add coordination after the expensive
work has already begun.

## Observed workflow boundaries

AMF already has these relevant transitions:

1. The feature wizard can enter `PlanInterview` before a feature exists. The
   interview collects a brief and static answers, then optionally runs adaptive
   rounds, synthesis, investigations, and revisions.
2. Synthesis produces a structured plan and stops at a `Review` phase. The
   user can edit, request critique, investigate a focus, request a directed
   revision, or accept.
3. Accepting a new-feature plan writes `AMF_PLAN.md`, persists the interview,
   and then creates/starts the feature. The kickoff prompt tells the new agent
   to read the approved plan.
4. An on-demand interview for an existing feature writes the plan and offers a
   user-controlled kickoff handoff to a live session. The session may already
   have work in progress, so automatic injection is unsafe.
5. Implementation and review already have failure/review surfaces, but their
   context is larger, more fragmented, and later in the cost curve.

The existing user-triggered plan critique is already close to the desired
preflight shape: it receives the draft plan, brief, answers, repository
context, and attached evidence, and it is advisory. The main missing decision
is when to offer or require it and which harness/profile should run it.

## Placement comparison

| Location | Benefit | Token risk | Recommendation |
| --- | --- | --- | --- |
| Before the interview | Can challenge an initial brief | Too little intent or acceptance detail; duplicates discovery | Do not use as the default |
| During adaptive rounds | May improve questions | Repeats on every round and pays before the shape of the work is known | Keep the existing planner; no automatic expert call |
| After synthesis, before acceptance | Complete plan plus evidence; prevents wasted implementation and revision work; can expose questions while correction is still cheap | One initial call plus at most one bounded follow-up; can be skipped by policy | **Preferred default** |
| After acceptance, before agent start | Final plan is stable and kickoff is available | Slightly later; still avoids implementation tokens | Fallback when the review gate is bypassed |
| On repeated implementation failure | Strong escalation signal and concrete failure evidence | Late, potentially includes retries and a large transcript | Trigger only on explicit local signals |
| During ordinary implementation | May rescue a difficult task | Hard to target, competes with the implementer, risks duplicated context | Avoid automatic invocation |
| Completion/review | Finds defects | Too late to reduce most implementation tokens | Use existing review tools, not Expert Assist |

## Proposed automatic policy

The preflight should evaluate deterministic signals before invoking a paid
expert:

- plan mode is enabled and synthesis produced a non-empty plan;
- the plan contains explicit acceptance criteria or validation steps;
- the feature is new, or the accepted plan changed materially since the last
  consultation;
- the task has risk indicators such as migrations, concurrency, public API,
  security, data loss, broad file scope, or an unresolved investigation;
- no equivalent consultation result is fresh for the same plan fingerprint,
  repository revision, and evidence packet.

If the signals do not justify the cost, AMF proceeds normally. If they do, the
user sees a short preflight notice with the expected purpose and bounded token
budget. The expert returns an implementation brief, not just a critique. Its
required sections are:

- **Objective and non-goals:** what the implementer must accomplish and what
  must remain untouched;
- **Ordered implementation steps:** the safest sequence, including dependency
  order and migration or compatibility ordering;
- **Code map:** relevant files, modules, symbols, data boundaries, and
  ownership points to inspect or change, with a reason for each target;
- **Invariants and decisions:** behavior that must remain true, the rationale
  for important choices, and alternatives that were rejected;
- **Validation plan:** focused tests, fixtures, commands, and observable
  acceptance checks, including failure and recovery paths;
- **Risks and stop conditions:** likely failure modes, unsafe assumptions, and
  when the implementer must pause for another review;
- **Definition of done:** a short checklist the implementer can verify before
  declaring the task complete.

The expert also returns blockers, assumptions to verify, confidence, and
optionally up to three clarification questions. Each question must state which
plan decision it unblocks and what evidence or choice is required. The expert
should spend its extra reasoning budget resolving ambiguity and making the
implementation sequence precise, rather than producing a broad speculative
patch or repeating the user brief. AMF never lets the expert edit the worktree.

When questions are returned, the review screen shows a short answer step. The
user can answer, skip a question, revise the plan directly, or continue without
answering. AMF then permits at most one follow-up review, which receives the
original packet, the expert findings, and the user's answers. A follow-up cannot
emit another question round; unresolved questions are recorded as assumptions
and the user may still accept the plan. Empty or unchanged answers do not
trigger a paid follow-up.

The accepted implementation brief is attached to the kickoff context for the
cheaper model. The implementer is told to follow the ordered steps, preserve
the listed invariants, run the validation plan, and report any stop condition.
The brief is a compact working contract: it reduces rediscovery and
backtracking without making the expert responsible for edits or silently
expanding the task.

The default should be one no-tools or tightly read-only review call. A second
call is reserved for the bounded clarification follow-up or another concrete
new signal, such as a materially changed plan or failed acceptance check. There
is never an automatic third call. Adaptive rounds and repository investigations
remain planner features; they should not silently multiply expert calls.

## Token and quality model

The relevant comparison is not expert-call tokens versus zero. It is:

```text
preflight cost + implementer cost after correction
versus
implementer cost + retries + failed validation + late review
```

The preflight pays off when it prevents even one materially wasteful attempt.
AMF should record plan fingerprint, expert usage, avoided/reported risk,
implementation retries, validation outcome, and accepted-change result. Cost
remains unknown until a concrete model and pricing snapshot are selected.
Quality must compare accepted changes against an expert-only or implementer-
only baseline; passing tests alone is insufficient.

## Reuse from the closed prototype

Keep the pieces that support this placement:

- typed execution policies and bounded owned jobs;
- evidence packets, omission markers, source hashes, and consultation-local
  usage export;
- immutable revisions and recovery semantics where a review may be resumed;
- prompt registry and pre-call metadata patterns;
- target validation only for explicitly requested handoffs.

Do not carry forward the default UX of a dashboard `ask-expert` form, a
separate post-hoc handoff, or automatic tmux delivery. Those are useful escape
hatches for a later explicit escalation flow, but they are not the efficient
default for a user who has just asked AMF to build something.

## Next implementation slice

The first implementation slice is now complete: the existing plan critique is
an automatic high-risk preflight, returns a structured implementation brief,
supports up to three clarification questions and one follow-up, and carries
the accepted brief into the implementation kickoff. The remaining work is a
configurable off/suggest/require policy, durable preflight result and plan
fingerprint persistence, lifecycle usage/outcome recording, and real quality
and cost evaluation before selecting model defaults.

Real provider conformance, pricing, and quality measurements remain pending.
