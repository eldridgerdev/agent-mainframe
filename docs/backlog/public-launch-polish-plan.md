# Public launch polish

- **Status:** Done — all six phases shipped
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

- [x] Document the **Pi** agent harness: intro, features list,
      prerequisites, quick start, session picker table, data model,
      and an Agent Support bullet (plain `pi` CLI; no vibe-mode flags,
      diff review, or usage meters).
- [x] Document the first-run **harness setup wizard** (Quick Start
      step 1 + `A` keybinding).
- [x] Add missing dashboard keybindings: `A`, `G`, `V`, `u`,
      `Ctrl+Space c`; leader table gained `N` (TODO quick-capture)
      plus the other help-overlay leader keys (`e`, `s`, `d`, `m`,
      `b`, `v`, `g`, `R`, `V`, `A`). Removed the stale `m` Memo row —
      the Memo feature no longer exists in the code.
- [x] Add sections for the v0.28–0.29 flagship features: Final
      Review (incl. AI co-reviewer and suggested changes), PR Comment
      Review, Per-Project TODO Lists, and Usage and Cost Meters; each
      also got a features-list bullet.
- [x] Prune stale upgrade caveats into a one-line troubleshooting
      note.
- [x] Sweep against the app. Also fixed beyond the audit list: config
      examples used capitalized `"Claude"`/`"Vibe"` agent/mode values
      that would fail to deserialize (serde expects lowercase);
      `feature_presets` docs dropped the removed `enable_notes` and
      gained `plan_mode`/`remote_control`; keybinding action list
      gained `syntax_picker`/`pr_review`/`session_config` and the
      unbound `next_feature`/`prev_feature`; config table gained
      `final_review_post_to_pr`, `view_auto_refresh`, `token_pricing`,
      `remote_control_default`; prompt library `leader+P` → `leader+p`
      and the "planned" placeholder note replaced (fill-in flow
      shipped); store version 4 → 5; contributing commands now match
      the CI gate; "Last updated" bumped.

## Phase 5 — First-run and dialog UX nits

All found by driving the TUI with a fresh HOME.

- [x] Harness wizard footer said `c confirm  Esc confirm` — two keys
      labeled confirm, and Esc conventionally cancels. Relabeled to
      `c/Esc done  q quit` for the first-run case (both keys really do
      the same thing there, since there's nothing to cancel back to);
      the non-startup "manage harnesses" case keeps the already-correct
      `c confirm  Esc cancel`.
- [x] New Project dialog footer said `Enter confirm`, but Enter
      advances Name → Path → Harness and only confirms on the last
      field. Relabeled to `Enter next` on the first two fields,
      `Enter confirm` on the last, matching the feature wizard
      convention.
- [x] Empty-state line `No projects yet. Press N to create one.` is
      flush against the left border — added the leading pad space
      every other row has.
- [x] `amf --help`: added a description for `-V, --version` and
      fleshed out the top-level about text.
- [x] Reviewed the three `(experimental)` toggles in the feature
      wizard (Review, Plan, Steering). Graduated Review and Plan —
      dropped the label there and from every other place it echoed
      (batch creation, feature-list badges, view-mode header badge,
      leader-key help, preset summaries). Left Steering flagged; it's
      still marked experimental in its own leader-key help entry and
      has ongoing feature churn.

## Phase 6 — Nice-to-have before/shortly after launch

- [x] Respect `XDG_CONFIG_HOME` in `amf_config_dir()`
      (`src/project.rs:1208`) instead of hardcoding
      `~/.config/amf` — now resolves via `dirs::config_dir()`, but
      falls back to the legacy `~/.config/amf` path when that already
      has data and the XDG-resolved directory doesn't, so existing
      installs (including macOS, where `dirs::config_dir()` differs
      from `~/.config`) keep working unchanged after an upgrade.
      Follow-up: Claude hook cleanup now also recognizes older AMF
      helper paths under any `.config/amf` root, so hooks written while
      `HOME` / `XDG_CONFIG_HOME` was temporarily redirected are removed
      instead of leaving deleted `/tmp/claude-*` scripts in
      `.claude/settings.local.json`.
      Follow-up: AMF-launched agent panes now export `AMF_ACTIVE=1`,
      and the local Claude/Codex hooks plus Opencode plugins exit when
      that marker is absent. This keeps AMF-managed hooks from blocking
      standalone harness runs in an AMF-prepared worktree.
- [x] Decide whether `docs/backlog/` (including the bug backlog)
      should ship in the public repo as-is; it's honest, but review it
      for anything you don't want public. Decided: ship as-is — the
      backlog's own README already frames it as an honest record of
      paused/planned work.
- [x] Tidy `scripts/`: separate dev/test helpers (`test-thinking.sh`,
      release/packaging tooling) from scripts users are expected to
      run, or document what each is for. Moved `release.sh`,
      `package-release-bundle.sh`, `package-no-tmux-test-bundle.sh`,
      `run-no-tmux-docker.sh`, `generate-amf-themes.{sh,js}`,
      `test-thinking.sh`, and `scripts/amf/` (PR helper scripts) into
      `scripts/dev/`; updated the CI release workflow, docker-no-tmux
      docs, and the `amf:pr-*` skill commands to match. Left the 10
      `include_str!`-embedded hook scripts (notify, thinking, tool,
      codex, session-status) at `scripts/` root since they're compiled
      into the binary, not run by contributors — added a README to
      each directory explaining the split.

## Verification

- `cargo test`, `cargo check`, and `cargo clippy -- -D warnings` all
  clean.
- Manual first-run pass with a scratch `HOME`: harness wizard → empty
  dashboard → create project → create feature, confirming the footer
  hints match actual key behavior.
- README keybinding table diffed against the in-app help overlay.
