# Crate package size

- **Status:** Backlog
- **Owner:** unassigned
- **Relates to:** the `publish-crate` CI job and `Cargo.toml` metadata added
  to enable crates.io publishing (see `.github/workflows/release.yml`,
  `Cargo.toml`'s `exclude`).

## Why / problem

The published package is far larger than it needs to be:
`cargo publish --dry-run` reports **703 files, 20.0MiB uncompressed /
11.1MiB compressed**. Inspecting `target/package/agent-mainframe-<ver>/`
shows most of that is not needed to build the `amf` binary:

- `docs/screenshots/` alone is **~11MB** — over half the package, and
  pure documentation imagery with no `include_str!`/`include_bytes!`
  reference from `src/`.
- `docs/backlog/` (776K), `docs/development/` (456K), and most other
  `docs/*.md` files are design notes, not build inputs.
- `.agents/`, `.claude/`, `.codex/`, `examples/`, `docker/`, `site/`
  (the new marketing/docs site), and most of `plugins/` ship in full but
  are irrelevant to compiling or running the binary.

The first real publish attempt (after the metadata fix landed) failed
with a `503 backend write error` from crates.io's Varnish layer — almost
certainly a transient issue on their end, since reads to the registry
worked fine and no partial crate was created. But an unnecessarily large
upload is more likely to hit exactly this kind of transient failure, and
there's no reason to ship 20MB of screenshots and internal dev tooling to
every `cargo install agent-mainframe` user regardless.

## Proposed design

`src/` legitimately pulls a handful of files from outside `src/` via
`include_str!`/`include_bytes!`, so the package can't just be `include =
["/src", ...]` — that was tried once already and broke the
`cargo publish --dry-run` verification build (it compiles the packaged
tarball, so a missing embed fails loudly, which is useful). The known
embed targets, as of this writing:

- `themes/opencode/*.json` (`src/theme.rs`)
- `skills/amf-add-session/SKILL.md`, `amf-add-hook`, `amf-add-preset`,
  `amf-add-prompt`, `amf-configure`, `amf-release-notes` (`src/app/setup.rs`)
- `scripts/*.sh` (`src/app/setup.rs`)
- `.opencode/plugins/*.js` (`src/app/setup.rs`)
- `plugins/diff-review/scripts/custom-diff-review.sh` (`src/app/setup.rs`)
- `docs/tsx-syntax-test.tsx`, `docs/syntax-tests/syntax-test-highlight.ts`
  (`src/highlight/tree_sitter.rs`, `src/ui/dialogs/diff.rs`)

So the right shape is an **exclude list of specific heavy,
non-embedded paths**, built and verified incrementally — not a
from-scratch include allowlist. `docs/screenshots/` is the single
biggest win and should be excluded first; the rest of `docs/` needs a
path-by-path check against the list above before exclusion (only two
files under `docs/` are actually embedded).

## Progress

- [ ] Exclude `docs/screenshots/` (biggest single win, ~11MB, no embeds).
- [ ] Audit remaining `docs/*` subpaths against the embed list above and
      exclude everything except `docs/tsx-syntax-test.tsx` and
      `docs/syntax-tests/`.
- [ ] Exclude `.agents/`, `.claude/`, `.codex/`, `examples/`, `docker/`,
      `site/`.
- [ ] Exclude `.opencode/*` except `.opencode/plugins/*.js`, and
      `plugins/*` except `plugins/diff-review/scripts/custom-diff-review.sh`.
- [ ] Re-run `cargo publish --dry-run` after each batch of excludes —
      it compiles the packaged tarball, so a missing embed fails
      immediately rather than at actual publish time.
- [ ] Record the final compressed size and confirm it's meaningfully
      smaller.
- [ ] Confirm an actual (non-dry-run) publish succeeds once the package
      is slimmed down, independent of whatever caused the 503.

## Open questions

- Was the `503 backend write error` actually size-related, or an
  unrelated transient crates.io issue? No way to confirm after the fact;
  treat this as good hygiene regardless, not a guaranteed fix.
- Does crates.io currently enforce a hard package size cap we're closer
  to than we'd like? Worth checking before assuming headroom.
- Keep `CHANGELOG.md`, `README.md`, `AGENTS.md`, `CLAUDE.md` in the
  package (small, conventional, useful on crates.io/docs.rs) — no open
  question there, just noting they're intentionally out of scope for
  trimming.

## Reasoning / when to build

Low urgency — doesn't block anything if the next publish attempt
succeeds on retry regardless of size. Worth doing before the crate is
publicly discoverable on crates.io, since first impressions there
(package size shown on the crate page, install time) are hard to walk
back later.
