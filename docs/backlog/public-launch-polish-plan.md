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

- [ ] PR review: triage / reply state is lost on return from a fix
      session. Repro and leads are in
      [bug-backlog-plan.md](bug-backlog-plan.md). This is the one item
      needing real investigation; it makes a flagship feature feel like
      it loses user data. Strike it through in the bug backlog when
      fixed.

## Phase 3 — Lint and build hygiene

Anyone installing from source sees build output; public Rust projects
get judged on `cargo clippy`.

- [ ] Fix the `dead_code` warning for `discover_source`
      (`src/token_tracking.rs:163`) — wire it up or remove it. This one
      prints on every plain `cargo build`.
- [ ] Clean up the ~80 clippy warnings (snapshot at audit time: 36
      collapsible `if`s, 6 unit let-bindings, 6 "items after test
      module", 2 identical `if` blocks, clamp patterns, useless
      conversions, etc.). Mechanical pass; `cargo clippy --fix` covers
      much of it.
- [ ] Decide on the `too_many_arguments` warnings (worst offender:
      15 args): refactor into param structs or add targeted `#[allow]`s
      with a comment.
- [ ] Add a CI gate (or at minimum a documented pre-release step) that
      runs `cargo clippy -- -D warnings` so the noise never comes back.

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
