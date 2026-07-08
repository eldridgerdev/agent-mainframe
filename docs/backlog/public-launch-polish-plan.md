# Public launch polish

- **Status:** In progress
- **Owner:** unassigned
- **Relates to:** repo metadata (`Cargo.toml`, `LICENSE`), docs
  (`README.md`), lint hygiene (whole crate), first-run and dialog UX
  (`src/ui/dialogs/`, `src/handlers/`), config path resolution
  (`src/project.rs`), PR review state
  ([bug-backlog-plan.md](bug-backlog-plan.md))

## Why / problem

AMF is about to be advertised publicly. A polish audit (2026-07-01,
v0.29.0) found the fundamentals healthy — 813 passing tests, clean
build, thorough CHANGELOG — but a handful of gaps that would hurt first
impressions: no license, README drift against shipped features, clippy
noise visible to anyone building from source, one data-loss-feeling bug
in a headline feature, and several small first-run UX rough edges.

This doc is the prioritized punch list. Check items off as they land
(with the commit/PR next to them where useful).

## Phase 1 — Launch blockers

Legal/metadata basics. Nobody can adopt or contribute without these.

- [x] Add a `LICENSE` file at the repo root (decided: MIT).
- [x] Add package metadata to `Cargo.toml`: `description`, `license`,
      `repository`, `readme`, `keywords`, `categories`.
- [x] Mention the license at the bottom of `README.md`.

## Phase 2 — Fix the headline-feature bug

- [x] PR review: triage / reply state is lost on return from a fix
      session. Fixed in PR #373 (`c41f465`, 2026-06-30): triage was
      keyed by `PR# + comment id + head_sha`, so the fix session's push
      moved the head and orphaned every mark; migration 010 re-keys on
      `PR# + comment id`. Struck through in
      [bug-backlog-plan.md](bug-backlog-plan.md).

## Phase 3 — Lint and build hygiene

Anyone installing from source sees build output; public Rust projects
get judged on `cargo clippy`.

- [x] Fix the `dead_code` warning for `discover_source`
      (`src/token_tracking.rs:163`) — it is exercised only by unit
      tests, so it got the same `#[allow(dead_code)]` annotation the
      file already uses for `new()`.
- [x] Clean up the ~80 clippy warnings. `cargo clippy --fix` handled
      the mechanical 65; the rest by hand (let-chain collapses,
      `clamp`, `map_while` on `lines()`, boxed the large
      `NewSessionTarget::Custom` variant, type aliases for the
      4-tuple/19-tuple signatures, moved 6 mid-file test modules to
      EOF). `cargo clippy --all-targets` is now warning-free.
- [x] Decide on the `too_many_arguments` warnings: added targeted
      `#[allow]`s with a one-line rationale at each of the 9 sites.
      The three `src/app/hooks.rs` worktree-hook functions share the
      feature-wizard field set and are the real param-struct candidates
      if this ever gets revisited.
- [x] Add a CI gate: the Lint job now runs
      `cargo clippy --all-targets -- -D warnings` and
      `cargo fmt --check` (was `-W clippy::all`, which never failed).

## Phase 4 — README refresh

Biggest single item. The README footer says "Last updated 2026-06-16"
and it has drifted from the app.

- [ ] Document the **Pi** agent harness (it appears in the first-run
      wizard and session picker but is absent from the README).
- [ ] Document the first-run **harness setup wizard**.
- [ ] Add missing dashboard keybindings to the table: `A` (manage
      harnesses), `G` (PR comment review), `V` (check pending diff
      review), `u` (preferred harness / worktree config), and the
      `Ctrl+Space c` config wizard; add leader `N` (TODO quick-capture)
      to the leader table.
- [ ] Add sections for the v0.28–0.29 flagship features currently
      missing entirely: per-project TODO lists, PR comment review, AI
      co-reviewer in final review, suggested changes, and the
      per-session usage/cost meters shown on the dashboard.
- [ ] Prune stale upgrade caveats (v0.10.1 TLS note, v0.11.1 404 note)
      into a short troubleshooting note or drop them.
- [ ] Sweep the rest of the README against the actual app (help
      overlay is the source of truth) and bump the "Last updated" date.

## Phase 5 — First-run and dialog UX nits

All found by driving the TUI with a fresh HOME.

- [ ] Harness wizard footer says `c confirm  Esc confirm` — two keys
      labeled confirm, and Esc conventionally cancels. Relabel (e.g.
      `c/Esc done  q quit`) or make Esc actually cancel.
- [ ] New Project dialog footer says `Enter confirm`, but Enter
      advances Name → Path → Harness and only confirms on the last
      field. Relabel to `Enter next` (matching the feature wizard) or
      make Enter confirm from any field.
- [ ] Empty-state line `No projects yet. Press N to create one.` is
      flush against the left border — add the leading pad space every
      other row has.
- [ ] `amf --help`: add a description for `-V, --version` and flesh out
      the top-level about text.
- [ ] Review the three `(experimental)` toggles in the feature wizard
      (Review, Plan, Steering): graduate the ones that are stable, or
      hide the ones that aren't ready — a wall of "experimental"
      undercuts the polish impression.

## Phase 6 — Nice-to-have before/shortly after launch

- [ ] Respect `XDG_CONFIG_HOME` in `amf_config_dir()`
      (`src/project.rs:1208`) instead of hardcoding
      `~/.config/amf` — use `dirs::config_dir()` with a migration
      fallback to the old path. Linux users will file this quickly.
- [ ] Decide whether `docs/backlog/` (including the bug backlog)
      should ship in the public repo as-is; it's honest, but review it
      for anything you don't want public.
- [ ] Tidy `scripts/`: separate dev/test helpers (`test-thinking.sh`,
      `vtcheck`-adjacent tooling) from scripts users are expected to
      run, or document what each is for.

## Verification

- `cargo test`, `cargo check`, and `cargo clippy -- -D warnings` all
  clean.
- Manual first-run pass with a scratch `HOME`: harness wizard → empty
  dashboard → create project → create feature, confirming the footer
  hints match actual key behavior.
- README keybinding table diffed against the in-app help overlay.
