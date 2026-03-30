# Steering Coach Spec

## Goal

Help AMF users launch new feature agents with a stronger draft task prompt.

## Minimal Slice

- add prompt coaching to the existing `CreatingFeature` dialog flow
- collect a draft task prompt before `create_feature()` runs
- score the prompt with lightweight local heuristics
- show missing constraints and concrete add-this guidance in the dialog
- allow launch even if the score is weak; the tool should coach, not block
- inject the finalized prompt into the launched agent session automatically

## Checklist Model

Score prompt quality out of 10, with 2 points each for:

1. file scope
2. acceptance criteria
3. invariants / non-goals
4. validation commands
5. risks / watch-outs

Each missing category should produce:

- a short explanation of what is missing
- a concrete suggestion the user can add to the prompt

## UI Touchpoint

Reuse the feature creation wizard:

1. source / branch / worktree / mode
2. existing supervibe confirm if applicable
3. worktree-created hook flow if applicable
4. create + start feature
5. steering prompt overlay on top of the running agent session
6. inject prompt into the agent

The steering prompt overlay should show:

- editable prompt text
- score summary
- present vs missing checklist items
- steering tips that teach what a stronger prompt usually includes

## Data Model

No persistence required for the first slice.

- `CreateFeatureState.task_prompt: String`
- `CreateFeatureState.prompt_analysis: PromptAnalysis`
- `CreateFeatureState.steering_enabled: bool` defaulting to `true`
- `SteeringPromptState` tied to the running agent view

## Non-Goals

- no new top-level mode or separate prompt coach subsystem
- no repo-specific learned guidance yet
