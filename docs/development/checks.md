# Development checks

Use current stable Rust with rustfmt and Clippy. AMF uses edition 2024 and builds
bundled SQLite/tree-sitter C code, so a C compiler/linker is required. The minimum
compiler version is not separately tested; CI uses stable.

## Local setup and iteration

The full suite runs on Unix and uses temporary Git repositories, shell/process
fixtures, Unix sockets and FIFOs. Install `git`, `tmux`, `sh`, `bash`, `ps`, `sleep`,
`mkfifo` and a C toolchain. On Ubuntu, CI installs `build-essential pkg-config git
tmux bash coreutils procps`; on macOS it uses the runner's Xcode/Unix tools and
installs tmux with Homebrew. No authenticated GitHub account or paid agent harness
is required for tests. Running AMF itself requires whichever of Claude Code,
Codex, OpenCode and Pi you select.

```sh
cargo build --locked
cargo test --locked app::tests::feature_sessions
cargo test --locked app::pr_review
cargo test --locked app::review
cargo test --locked app::learning
```

For a code milestone, run the normal parallel suite and both checks:

```sh
cargo test --workspace --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
```

The WSL-only clipboard integration test accesses the real Windows clipboard.
Run with functioning WSL interop. A restricted sandbox can prevent Unix socket
binding or clipboard access; rerun in an environment providing those facilities
and report the restriction rather than ignoring or serializing tests.

## Adding or moving code and tests

Use [architecture.md](architecture.md) to choose the owner. Add App integration
coverage to the appropriate `src/app/tests/` suite; put pure unit coverage beside
the behavior it tests. Share only fixtures with actual multi-suite consumers in
`tests/support.rs`, with test-only narrow visibility. Keep temporary directories,
DB handles and process guards alive for the whole test. Use `App::new_for_test`
with MockTmuxOps/MockWorktreeOps; new feature groups should use the same defaults
in production and tests. Do not make production APIs public to accommodate moves.

Keep pure transformations explicit about their inputs. App orchestration owns
mode changes and calls existing DB/external-operation boundaries. When changing
worker ownership, exercise closing with pending work, switching target/session,
reopening, and stale or disconnected results. Tests for an existing regression
should remain intact; do not add tests merely for a file's location.

For this refactor's preservation check, capture `cargo test --workspace --locked
-- --list`. Transform the original names in [baseline/tests.txt](baseline/tests.txt)
through [the M2 mapping](baseline/app-tests.tsv) and
[the state mapping](state-test-paths.tsv), then compare the full sets and counts.
The sole M4 addition is
`app::tests::pr_triage::closed_fetch_cannot_replace_a_reopened_pr_with_its_queued_result`.
All 2,511 originals remain accounted for (2,512 total).

Never use `println!`/`eprintln!` in TUI code. Use App's `log_debug`, `log_info`,
`log_warn`, or `log_error` with a short context such as `sync` or `worktree`.
Dashboard `D` opens the log; the file is `~/.local/state/amf/debug.log`.

## CI jobs

[main.yml](../../.github/workflows/main.yml) runs on PRs targeting main/master and
pushes to main/master/testing. Linux retains the required-check name `Test`;
macOS adds `Test (macOS)`. Both execute the full locked parallel suite with
explicit prerequisite installation/checks. `Lint` runs once on Linux. Existing
check names and triggers remain; the workflow does not change branch protection.
Maintainers can require the new macOS/audit checks after verifying their runs.

`Dependency audit` runs on those PRs/pushes and Mondays at 07:17 UTC. Only the audit
job runs on the schedule. Workflow permissions are `contents: read`; the audit
job does not create issues, submit check annotations through a write token, or
modify the lockfile.

The tool is pinned to
[cargo-audit 0.22.2](https://github.com/rustsec/rustsec/releases/tag/cargo-audit%2Fv0.22.2),
using the [RustSec CLI installation](https://github.com/rustsec/rustsec/tree/main/cargo-audit)
with locked tool dependencies. A plain CLI job keeps permissions read-only and
makes the local command identical to CI:

```sh
cargo install cargo-audit --version 0.22.2 --locked
cargo audit --file Cargo.lock
```

Audit reads the existing lockfile and fetches the current RustSec database; it
has no `--locked` option. Known vulnerabilities fail the job. Advisory warnings
(including unmaintained, unsound and yanked dependencies) remain visible in the log even when
they are not fatal. The audit build uses a C toolchain, pkg-config and OpenSSL
headers. No advisory exceptions are configured by this change. The local scan and
remaining warning follow-ups are recorded in [advisories.md](advisories.md).

The repository maintainer owns advisory triage: identify the affected dependency
path and exposure, open a focused fix, and re-run the audit. Keep dependency
upgrades separate from refactors. If an exception is necessary, record its exact
advisory ID, rationale, owner, tracking issue and expiry in `.cargo/audit.toml`
and these notes; review it on every scheduled finding and remove it when fixed
or expired. Do not silence all warnings or use a blanket ignore.

## Verification status

Final local validation on 2026-09-08 passed all 2,512 tests (22.82s, no failures
or ignored tests), formatting, strict locked all-target Clippy, Actionlint 1.7.7,
and local documentation-link checks. The patched lockfile audit exited 0 with
zero blocking findings and five documented nonfatal warnings.

Local Linux/WSL baseline and inventory evidence are in
[baseline/README.md](baseline/README.md). Hosted Linux/macOS/audit runs must be
inspected for the published commit before CI is called green. Workflow linting
and a local audit do not establish macOS execution or hosted success. Publishing
this work remains a separate authorized step under `AMF_PLAN.md`.
