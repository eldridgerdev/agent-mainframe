# Agent limits and resource health

Implementation notes for agents and contributors. Source is authoritative
when it disagrees; update this file in the same change.

## Agent Limits & Resource Health (resources/)

Guards against AMF quietly exhausting host memory. Everything here
is advisory: a tripped gate asks, it never refuses, and a missing
signal is always "no gate" rather than a block.

```text
resources/
├── mem.rs      # probe() -> Option<MemorySnapshot>: /proc/meminfo
│               # narrowed by cgroup v2 limits on Linux, sysctl +
│               # vm_stat on macOS, None everywhere else
├── limits.rs   # LiveHarnesses census + active_harness_sessions();
│               # HeadlessLease counts in-flight headless runs
├── procs.rs    # ps-backed process list/tree, pid liveness,
│               # SIGTERM-then-SIGKILL tree termination, VS Code
│               # window attribution
└── doctor.rs   # `amf doctor` checks + text/JSON rendering
                # (reads via AmfDb::open_read_only + setup::read_config:
                #  no file creation, no migration, no journal change)
```

- **Pre-start gate** (`app/resource_gate.rs`): `check_start_preconditions()`
  combines the agent count (harness sessions across all projects +
  headless leases, tripping at `active >= limit`) and available
  memory into one result. The gate lives in the **launch primitives**
  (`ensure_feature_running`, `ensure_feature_running_for_new_session`,
  `create_agent_session_labeled`, and the three
  `ensure_feature_running_with_*_session` pickers), not on entry points:
  each takes a `StartIntent` so a new caller must pick a policy.
   - `Approved` — an upstream gate already cleared this start.
   - `Ask(PendingStart)` — park in `AppMode::ConfirmResourceStart`,
     replayed by `confirm_pending_start()`. Used by the dashboard start,
     session adds, `enter_view`, and `switch_view_to_feature`.
   - `Warn(&str)` — toast and proceed, for flows whose resume state lives
     in the `AppMode` the dialog would replace (TODO spawn, PR triage,
     final review, saved-transcript pickers).

  The gate only fires when the call will actually launch, so re-entering a
  running feature never asks. Creation paths call `autostart_allowed()`
  instead, which warns and skips rather than prompting.
- **Headless accounting**: `HeadlessLease` is acquired inside
  `run_command` / `run_jsonl_command` (`headless.rs`), so every
  `HeadlessRunner` caller is counted; poll-driven runs hold a
  `LeasedChild` instead. Dropping one **kills** the run: it terminates the
  process tree and reaps it on a background thread, holding the lease until
  the process is really gone (`std::process::Child` only detaches on drop,
  which would leave an abandoned harness running but uncounted).
- **Editor tracking**: `launched_editors` (`MIGRATION_017`, plus
  `proc_started_at` in `MIGRATION_018`; `db/editors.rs`). VS Code launches
  with `--new-window` and is recorded **not-owned**; a background thread
  then attributes the new window process (new PID + worktree in argv) and
  only then marks it `dedicated`, storing the process's own start time.
  Reusing a running VS Code produces no new process, so that launch stays
  not-owned for good. `app/editor_ops.rs` revalidates identity before
  signalling — argv matched on **path boundaries** plus that start time, so
  a recycled PID never passes — and skips a window whose process is hosting
  more than one window (`procs::vscode_window_count`), because VS Code is a
  singleton and the others are the user's. It kills the process tree and
  returns a killed/skipped/pending report used by `do_stop_feature` and the
  dormant overlay.
- **Launch/stop race**: a stop during the seconds before attribution has
  nothing to kill, so `App::pending_editor_launches` hands the job over:
  `kill_tracked_editors` flips the launch's `PendingLaunchState` to
  `Reclaim` and the resolver closes the window it finds instead of recording
  it. The resolver holds the launch's mutex across deciding *and* writing,
  so either the stop claims it or the stop finds the row already owned.
- **Dormancy** (`app/dormant.rs`): idle (tmux `window_activity`) **and**
  unattended (`Feature::last_accessed`), both configurable; `z` opens
  `AppMode::Dormant`.
