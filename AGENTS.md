# AGENTS.md

Guidance for AI coding agents working in this repository.

## Build and validation

The package is `agent-mainframe`; its binary is `amf`. Use the current stable
Rust toolchain (edition 2024), with a C compiler for bundled SQLite/tree-sitter.

```bash
cargo build --locked
cargo run --locked
cargo test --locked app::tests::feature_sessions
cargo test --locked app::pr_review
cargo test --workspace --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
```

The repository has unit and integration tests. Choose a focused suite while
iterating; run the full parallel suite, formatting and strict Clippy at a code
milestone boundary. Do not ignore or serialize tests to hide a regression.
See [development checks](docs/development/checks.md) for prerequisites, CI,
dependency-advisory triage and the exact baseline comparison procedure.

## Runtime and architecture

AMF is a Rust TUI built on ratatui/crossterm/vt100. It supports Claude Code,
Codex, OpenCode and Pi sessions in tmux. Source installations need `tmux` on PATH;
install the harnesses you intend to use. Tests mock paid harness execution.

Use [the architecture guide](docs/development/architecture.md) to choose a module.
`main.rs` owns startup, event deadlines and polling; `handlers/` dispatches input;
`ui/` renders; `app/` orchestrates feature workflows; `db/` owns persistence.
PR Triage, Final Review and Learning each have a directory with owned state.
Central AppMode remains the routing boundary. Feature test suites live under
`src/app/tests/`, with shared fixtures in `support.rs`; small unit suites stay
with their implementation.

Preserve persistence formats, keybindings, worker identity checks and process
lifetimes during refactors. Keep pure transformations independent of App. Inject
tmux/worktree operations through `TmuxOps`/`WorktreeOps`; do not add persistence or
process launching to renderers. Keep temporary directories, databases and child
guards owned by each test, using `App::new_for_test` and group defaults.

State is persisted in the global SQLite store at `~/.config/amf/amf.db` for every
checkout. Tmux sessions use the `amf-` prefix. Configuration controls leader-key
and refresh timings; consult the code/config instead of assuming fixed intervals.

### Debug Logging

**NEVER use `println!` or `eprintln!` in TUI code** - it corrupts
the terminal display. Use the built-in debug log instead.

To view the debug log at runtime, press `D` from the dashboard.

**Log file location:** `~/.local/state/amf/debug.log`

You can tail this file in a separate terminal:
```bash
tail -f ~/.local/state/amf/debug.log
```

**Usage in code:**

```rust
// From anywhere with access to `app`:
app.log_debug("context", format!("value: {}", value));
app.log_info("context", "operation completed".to_string());
app.log_warn("context", "something unexpected".to_string());
app.log_error("context", format!("failed: {}", err));
```

**Log levels** (color-coded in UI):
- `DEBUG` (gray) - detailed tracing
- `INFO` (green) - normal operations
- `WARN` (yellow) - unexpected but handled
- `ERROR` (red) - failures

**Context strings** should be short identifiers like:
- `"amf"` - app lifecycle
- `"sync"` - status sync operations
- `"tmux"` - tmux interactions
- `"worktree"` - git worktree operations
- `"hooks"` - lifecycle hooks

Errors from `show_error()` are automatically logged to the
debug log with level ERROR.

### Local harness configuration

- **Never modify `~/.claude/settings.json` (global) or
  `~/.config/opencode/` (global opencode config) to inject
  hooks or settings.** Instead, write to the worktree's
  local `.claude/settings.local.json` (or `.opencode/` equivalent)
  via `ensure_notification_hooks()`. For non-worktree
  features (first feature that uses the repo dir directly),
  write to `{repo}/.claude/settings.local.json`. On startup,
  `cleanup_global_hooks()` actively removes any
  previously-injected global entries.

## Screenshot proof publication

Use the repository `amf-screenshot` skill only when the user explicitly asks
for visual proof. When the user also explicitly asks to publish that proof to
an open PR, use `scripts/dev/screenshot/publish-pages.sh --strict` after the
ref and scenario are pushed. The publisher requires the `eldridgerdev` GitHub
identity and writes only the marked PR-body region. It dispatches the isolated
capture workflow (the only job that checks out the ref), then downloads that
run's rendered frames and deploys the private Cloudflare Pages gallery from the
local machine — so it needs `CLOUDFLARE_API_TOKEN` in the environment (the owner
keeps it in `~/.secrets/cf-amf-pages.env`, sourced by the
`amf-publish-screenshots` shell wrapper; don't run `wrangler login`, it is
unreliable here), `CLOUDFLARE_ACCOUNT_ID` set, plus `wrangler` (or
`npx`). There is no `screenshot-pages`
environment or per-run approval: the Pages credentials never enter CI, and the
capture-vs-deploy split keeps that safe. Do not claim publication succeeded
until the command does. Raw ANSI/text captures and Actions artifact URLs are
internal and must not be placed in the PR.
