# Expert plan review: placement investigation

## Conclusion

The highest-value insertion point is the plan interview's final review gate,
after synthesis and before acceptance or implementation. At that point AMF has
the user's brief, answered discovery questions, a concrete plan, repository
context, and attached evidence. Advice can still change the implementation
approach before a cheaper model spends tokens following it.

The action must be explicitly requested for each plan. The comparison run for
the automatic prototype showed that an Expert call can improve instructions,
but it always adds its own context and output cost and does not guarantee that
the avoided implementation work will exceed that cost. Automatic admission
also hid a more basic problem: the UI exposed an ordinary agent review and the
runner used the harness default model. That did not prove a frontier model had
been consulted. The automatic `off`/`suggest`/`require` policy was removed.

## Workflow

1. Plan mode collects the brief and discovery answers, then synthesizes a
   concrete implementation plan.
2. At the final review gate, the user may press `a` for **Expert review**.
3. AMF requires an explicit frontier model for the plan's resolved harness.
   The user selects from the same verified presets used by AI Review, with
   `Custom…` available for unlisted IDs. A `review_models.plan_preflight` value
   may highlight the matching row, but the user must still confirm it. The
   shared ordinary `review_model` is never used.
4. The normal pre-call confirmation shows the prompt, harness, model, and token
   estimate. Only another explicit Enter starts the call.
5. The Expert reviews the plan read-only and returns a compact implementation
   brief. It may also ask up to three questions.
6. The user can answer those questions and request one bounded follow-up. It
   reuses the same harness and model and cannot create another question round.
7. If the plan is accepted, the Expert brief is included in the kickoff prompt
   for the cheaper implementation model.

Escape from model selection or pre-call confirmation spends no tokens and
leaves the plan unchanged. Editing or regenerating the plan invalidates review
findings for the superseded plan. Reopening a completed review does not make a
second call.

## Expert output contract

The Expert is asked to use its additional reasoning capacity to reduce
rediscovery and rework for the implementation model. Its response includes:

- objective and non-goals;
- ordered implementation steps and dependency order;
- a code map with relevant files, symbols, and ownership boundaries;
- invariants, settled decisions, and rejected alternatives;
- focused validation commands and observable acceptance checks;
- risks, unsafe assumptions, recovery paths, and stop conditions;
- a short definition-of-done checklist;
- blockers, confidence, and up to three clarification questions when needed.

Each clarification question states the decision it unlocks and the evidence or
choice required. The Expert reviews and advises; it does not edit the worktree
or silently expand scope.

## Placement comparison

| Location | Benefit | Cost risk | Decision |
| --- | --- | --- | --- |
| Before discovery | Can challenge the initial brief | Too little context; duplicates discovery | Do not add |
| During adaptive questions | May improve individual questions | Repeated calls multiply spend | Keep the existing planner |
| After synthesis, before acceptance | Full plan and evidence; changes are still cheap | One bounded review plus optional follow-up | Use as the explicit Expert action |
| During implementation | Can rescue difficult work | Duplicates context and arrives after spend | Use separate manual escalation if needed |
| Completion review | Finds defects | Too late to reduce implementation cost | Keep existing review tools |

## Measurement

The useful comparison is:

```text
Expert review cost + implementation cost after correction
versus
implementation cost + retries + failed validation + late review
```

AMF stores the reviewed plan fingerprint, lifecycle status, selected model,
token estimate, and whether an implementation brief was produced. This makes
the consultation attributable without claiming savings that were not measured.
Quality still needs scenario comparisons against an implementation-only
baseline; a passing test suite alone does not establish that the extra call was
worth its tokens.

## Implementation status

The plan-review action is user initiated and requires an explicit selection
from the shared model picker. The selected model is visible before dispatch,
reaches the headless runner and the single clarification follow-up, persists
with the plan interview, and appears in evaluation export. The structured brief
and kickoff handoff from the first prototype remain because they directly
improve the cheaper model's input.
