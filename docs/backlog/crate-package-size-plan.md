# Crate package size

- **Status:** Shipped
- **Owner:** unassigned
- **Relates to:** the `publish-crate` CI job and `Cargo.toml` metadata added
  to enable crates.io publishing (see `.github/workflows/release.yml`,
  `Cargo.toml`'s `exclude`).

## Why / problem

The published package was far larger than it needed to be:
`cargo publish --dry-run` reported **703 files, 20.0MiB uncompressed /
11.1MiB compressed**. Inspecting `target/package/agent-mainframe-<ver>/`
showed most of that wasn't needed to build the `amf` binary:

- `docs/screenshots/` alone was **~11MB** — over half the package, and
  pure documentation imagery with no `include_str!`/`include_bytes!`
  reference from `src/`.
- `docs/backlog/` (776K), `docs/development/` (456K), and most other
  `docs/*.md` files are design notes, not build inputs.
- `.agents/`, `.claude/`, `.codex/`, `examples/`, `docker/`, `site/`
  (the new marketing/docs site), and most of `.opencode/` shipped in full
  but are irrelevant to compiling or running the binary.

This turned out not to be cosmetic: the real publish attempts (after the
metadata fix landed) consistently failed with a `503 backend write
error`. Confirmed root cause via
[rust-lang/crates.io#10098](https://github.com/rust-lang/crates.io/issues/10098):
crates.io enforces a **hard 10MB cap on the compressed `.crate` file**;
an oversized upload gets a `413 Payload Too Large` from their API, but
their CDN layer (Heroku/CloudFront/Varnish, depending on which part of
the stack) mangles that clean rejection into an opaque `503` before it
reaches `cargo`. Our compressed size (11.1MiB) was just over that cap —
this is almost certainly why every publish attempt failed identically.

## Proposed design

`src/` legitimately pulls a handful of files from outside `src/` via
`include_str!`/`include_bytes!`, so the package can't just be `include =
["/src", ...]` — that was tried once and broke the `cargo publish
--dry-run` verification build (it compiles the packaged tarball, so a
missing embed fails loudly, which is useful). The known embed targets,
as of this writing:

- `themes/opencode/*.json` (`src/theme.rs`)
- `skills/amf-add-session/SKILL.md`, `amf-add-hook`, `amf-add-preset`,
  `amf-add-prompt`, `amf-configure`, `amf-release-notes` (`src/app/setup.rs`)
- `scripts/*.sh` (`src/app/setup.rs`)
- `.opencode/plugins/*.js` (`src/app/setup.rs`)
- `plugins/diff-review/scripts/custom-diff-review.sh` (`src/app/setup.rs`)
- `docs/tsx-syntax-test.tsx`, `docs/syntax-tests/syntax-test-highlight.ts`
  (`src/highlight/tree_sitter.rs`, `src/ui/dialogs/diff.rs`)

So the right shape was an **exclude list of specific heavy,
non-embedded paths**, not a from-scratch include allowlist.

## Progress

- [x] Excluded `docs/screenshots/` (biggest single win, ~11MB, no embeds).
- [x] Audited remaining `docs/*` subpaths against the embed list above and
      excluded everything except `docs/tsx-syntax-test.tsx` and
      `docs/syntax-tests/` (`docs/automation`, `docs/backlog`,
      `docs/development`, and the standalone `docs/*.md` design notes).
- [x] Excluded `/.agents`, `/.claude`, `/.codex`, `/examples`, `/docker`,
      `/site`.
- [x] Excluded `.opencode/commands`, `.opencode/opencode.json`,
      `.opencode/themes` (keeping `.opencode/plugins/*.js`, the only
      embedded part). `plugins/` needed no exclusion — it already
      contains only the one embedded script.
- [x] Re-ran `cargo publish --dry-run` after the change — verification
      build compiled clean, and every embed target
      (`for f in ...; do test -f target/package/.../$f; done`) confirmed
      present in the packaged tree.
- [x] Final size: **385 files, 8.2MiB uncompressed / 1.7MiB compressed**
      (down from 703 files / 20.0MiB / 11.1MiB) — comfortably under the
      10MB compressed cap with margin for growth.
- [ ] Confirm an actual (non-dry-run) `cargo publish` succeeds now that
      the package is under the cap. Expected to fix the `503`s, but not
      yet re-attempted after this change.

## Open questions

- None outstanding on the packaging side. The one open item is
  confirming the live publish succeeds (see Progress).
- Keep `CHANGELOG.md`, `README.md`, `AGENTS.md`, `CLAUDE.md` in the
  package (small, conventional, useful on crates.io/docs.rs) —
  intentionally out of scope for trimming.
- `examples/vtcheck.rs` is now excluded along with the rest of
  `/examples`, so `cargo publish` prints a harmless
  "ignoring example `vtcheck`" warning. It's a standalone dev diagnostic,
  not part of the installed binary, so this is expected.

## Reasoning / when to build

Done — this was blocking real publishes, not just a size nice-to-have.
