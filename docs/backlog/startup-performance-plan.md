# Startup performance

- **Status:** In progress
- **Owner:** unassigned
- **Relates to:** startup event loop (`src/main.rs`), application setup
  (`src/app/mod.rs`), usage refresh (`src/usage.rs`), token tracking
  (`src/token_tracking.rs`), Codex session index (`src/codex.rs`),
  sidebar session readers (`src/app/*_sessions.rs`), shipped PR
  [#308](https://github.com/eldridgerdev/agent-mainframe/pull/308)

## Why / problem

AMF was slow both before the first dashboard frame and for several
minutes afterward. The initial investigation found two distinct forms
of degradation:

- A hard startup stall before `App::new()` returned.
- Severe input and redraw latency after the dashboard appeared while
  background caches warmed.

The debug log captured a representative 38-second pre-dashboard gap:
the filesystem watcher and tmux observer were ready at `12:07:38`, but
AMF did not report startup complete until `12:08:16`. During the warm-up
period, input-to-draw latency reached 1.62 seconds and UI draws reached
609 ms.

The root causes were synchronous parser repair that could never succeed
in static builds, unbounded per-feature warm-up threads, and several
subsystems independently walking or parsing the same transcript trees.

## Design principles

- The dashboard should become interactive after only short bootstrap
  work. Expensive cache population belongs in background workers.
- Background work must be bounded and prioritized; feature count must
  not translate directly into thread count.
- Prefer provider indexes and exact transcript paths over recursive
  filesystem discovery.
- Preserve compatibility fallbacks for older provider versions that do
  not expose an index.
- Persist incremental parse state so restarts process only changed or
  appended transcript data.
- Measure worker execution and queueing, not only thread creation and
  result application.

## Progress

### Shipped

- [x] Detect whether the AMF binary supports dynamic loading before
  validating installed tree-sitter parsers.
- [x] Skip parser validation and automatic repair when dynamic loading
  is unavailable, eliminating the repeated, guaranteed-failure rebuild
  loop responsible for the measured 38-second stall.
- [x] Replace one-thread-per-feature sidebar warming with a lazy,
  bounded four-worker queue.
- [x] Discard queued sidebar warm-up work during shutdown while allowing
  active jobs to finish.
- [x] Add regression tests for unsupported dynamic loading and bounded
  sidebar concurrency.

These changes shipped in PR #308.

### Implemented on the current branch

- [x] Read Codex session ids, timestamps, and exact `rollout_path`
  values from `~/.codex/state_5.sqlite` using a read-only connection.
- [x] Use the Codex index for token-source discovery and exact transcript
  lookup instead of recursively enumerating every Codex JSONL file.
- [x] Route exact-session Codex prompt, title, model, and sidebar reads
  to the indexed rollout file, retaining recursive scans as a fallback.
- [x] Include indexed rollout metadata in sidebar cache signatures so
  appended transcript data invalidates the cache.
- [x] Sequence cold startup I/O: session-token refresh first, usage
  statistics second, and sidebar warming last.
- [x] Keep the dashboard interactive while the sequenced background
  work runs, while suppressing periodic refreshes until initial warming
  completes.
- [x] Add tests for Codex index filtering, exact path resolution, and
  startup ordering. The full suite currently passes (552 tests).

## Remaining backlog (ranked)

### 1. Consolidate Claude and opencode transcript processing

- [ ] Inventory overlapping reads between usage totals, per-session
  token status, sidebar prompts/models, and thinking-state fallbacks.
- [ ] Introduce a shared provider-specific transcript snapshot or index
  where multiple consumers need the same metadata.
- [ ] Preserve append-only offsets from the existing token cache rather
  than reparsing unchanged JSONL or storage files.

### 2. Avoid full reads of large target transcripts

- [ ] Change latest-prompt and latest-model readers to bounded tail
  reads where provider formats make that safe.
- [ ] Persist compact prompt/model metadata when a tail read cannot
  reliably reconstruct the latest state.
- [ ] Add large-transcript fixtures that prove work scales with appended
  bytes rather than total history size.

### 3. Prioritize visible work during sidebar warming

- [ ] Queue the selected feature and expanded project rows first.
- [ ] Defer stopped features and collapsed projects until the initial
  visible set is ready or the event loop is idle.
- [ ] Reprioritize queued work when navigation changes the visible set.

### 4. Reduce Codex index connection overhead

- [ ] Measure per-request read-only SQLite connection cost after indexed
  lookup lands.
- [ ] If material, share a short-lived immutable index snapshot or a
  dedicated index worker rather than opening a connection per lookup.
- [ ] Keep database access off the UI thread and tolerate Codex schema
  changes by falling back to filesystem discovery.

### 5. Instrument actual background costs

- [ ] Record session-status, usage-statistics, and sidebar worker
  execution duration rather than only scheduling duration.
- [ ] Record sidebar queue depth, wait time, active worker count, files
  visited, and bytes parsed.
- [ ] Add a startup completion summary that distinguishes time to first
  interactive dashboard from time to fully warm caches.

### 6. Move remaining synchronous initialization off the critical path

- [ ] Measure DB migration/store loading, token-cache deserialization,
  debug-log loading, filesystem watcher registration, notification
  pruning, and tmux compatibility checks independently.
- [ ] Move only proven expensive, nonessential operations behind the
  first interactive frame.
- [ ] Keep state required for correct initial navigation and session
  status synchronous.

### 7. Profile notification and watcher setup at scale

- [ ] Benchmark stale-notification pruning with large directories.
- [ ] Benchmark per-feature watch reconciliation with hundreds of
  features and worktrees.
- [ ] Make pruning incremental or background-only if either operation
  becomes visible in startup profiles.

## Success criteria

- Time to an interactive dashboard is not proportional to the number of
  installed syntax parsers, historical transcripts, or stored features.
- Startup background work uses a fixed concurrency bound.
- Input-to-draw latency remains below 100 ms during cold cache warming
  on a representative large store.
- A warm restart performs no recursive Codex transcript walk and reads
  only changed/appended provider data.
- Performance logs identify the responsible worker and queue delay when
  a startup regression occurs.

## Open questions

- Should cache warming finish every stored feature, or stop after the
  visible/active set and rely on watcher events plus lazy loading?
- Is one shared transcript service worth the coupling, or should each
  provider expose a small immutable index consumed by independent
  subsystems?
- Which startup metrics should be enabled by default without recreating
  the debug-log churn that previous performance logging caused?
