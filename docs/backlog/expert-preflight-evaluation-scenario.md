# Expert preflight realtime comparison

This scenario compares the same cheaper implementer with automatic Expert
preflight disabled, risk-triggered, and required. Each run starts from the
same clean commit in a fresh worktree with the same harness, model, prompt,
tool policy, and resource limits.

## Task

Add a persisted per-feature **implementation priority** with values `normal`,
`high`, and `urgent`.

The feature must:

- add the field to the SQLite feature record with a backward-compatible
  migration and a safe default for existing rows;
- expose the value in the feature create/edit flow;
- show the priority in the dashboard row;
- preserve it through save, reload, feature rename, and worktree operations;
- reject unknown serialized values without corrupting the database;
- include focused migration, persistence, and UI tests;
- leave all existing behavior unchanged when the value is `normal`.

The task is intentionally small enough for one implementation pass but crosses
schema, state, UI, compatibility, and test boundaries. It should trigger the
`suggest` policy through the migration/schema/backward-compatibility markers.

## Run protocol

1. Pin one AMF commit and record the exact implementer and Expert model IDs.
2. Create three clean worktrees from that commit: `off`, `suggest`, and
   `require`.
3. Give each implementer the identical task text above and no additional
   hints. Set `off` to skip preflight, `suggest` to risk-triggered preflight,
   and `require` to always preflight.
4. Save the plan and implementation transcript for every run. Do not manually
   repair a run; a failed run is part of its measured result.
5. Run the same validation command in every worktree:

   ```text
   cargo fmt --check && cargo clippy -- -D warnings && cargo test -- --test-threads=2
   ```

6. Have a blinded reviewer apply the same acceptance checklist to each diff.
7. Export the preflight record with
   `AmfDb::export_plan_preflight_evaluation(feature_id)` and attach it to the
   run record.

## Measurements

Record input/output tokens and elapsed time for planning, each Expert call,
implementation, retries, and validation. Also record:

- whether the run passed the full validation command;
- number and severity of reviewer findings;
- migration correctness and backward-compatibility result;
- number of implementation retries or reverted attempts;
- accepted diff size and changed-file count;
- preflight status, fingerprint, token estimate, and whether a brief reached
  kickoff;
- estimated cost using the same pricing snapshot for all runs.

The primary comparison is accepted quality at total cost, including failed
validation and retries. A preflight is beneficial only when its extra cost is
offset by fewer retries or materially better accepted quality. One task is a
smoke test, not a model-selection result; repeat with at least five tasks that
cover migrations, concurrency, security boundaries, and compatibility.

