# M1 validation baseline

Captured 2026-09-06; unrestricted validation completed 2026-09-07 at revision
`c4088b70cd22393dd055dd249816da6e0f6825f0`. The worktree was clean before M1.
Only documentation and inventories changed; no production code or tests moved.
Validation host: Linux `6.18.33.2-microsoft-standard-WSL2`, rustc `1.98.0`
(`88d9e12ae`), Cargo `1.98.0` (`797e8a9bc`).

| Command | Result |
| --- | --- |
| `cargo test --workspace --locked -- --list` | Passed; 2,511 named tests, zero benchmarks; complete stdout in `tests.txt` |
| `cargo test --workspace --locked` | Passed outside the sandbox with the normal parallel runner: 2,511 passed, 0 failed, 0 ignored, 0 filtered; 21.66 seconds |
| `cargo fmt --check` | Passed, no output |
| `cargo clippy --all-targets --locked -- -D warnings` | Passed, no warnings |
| Inventory consistency check | All 742 `app::tests::*` names in the runnable inventory have exactly one planned destination; no duplicate destinations |

The initial sandboxed full run reported 2,503 passed and eight failures: six IPC
socket tests and `tmux::tests::removes_stale_socket_files` could not bind sockets
(`Operation not permitted`); `app::util::tests::wsl_clipboard_round_trips_image_and_text`
could not access a clipboard utility. The unrestricted rerun passed every test.
These are recorded as environment constraints, not fixed or skipped tests.
The WSL test uses the real Windows clipboard; a full local run needs working WSL
interop. No remaining validation blocker was found.

## Inventory files

- [tests.txt](tests.txt): original `cargo test -- --list` stdout, preserving all
  full paths, including suites outside `app/tests.rs`.
- [app-tests.tsv](app-tests.tsv): 742 original test paths, baseline source lines,
  and explicit planned M2 destinations. This is a move map, not evidence that a
  move has happened.
- [test-helpers.tsv](test-helpers.tsv): 114 top-level helper functions, constants
  and guards from `app/tests.rs`, proposed destinations and lexical callers.
- [app-fields.tsv](app-fields.tsv): all 130 App fields, source lines, owner,
  declared type and files mentioning the field name. Channels are included;
  matching tokens in other types/comments can appear in the referencing list.

The ownership decisions and module responsibilities are in
[architecture.md](../architecture.md). Line numbers refer to the baseline only.
The inventories were derived from top-level declarations and lexical references,
with test paths checked against Cargo's runnable list. They do not replace
semantic inspection during extraction.

For M2, compare every original path in `tests.txt` against the new list: transform
central test paths through `app-tests.tsv`, leave all other paths unchanged, and
compare sets and cardinality. Update an intended destination explicitly if the
move exposes a better boundary. Record any added/removed tests by name and reason;
do not accept a matching count as proof of preservation. Keep the baseline files
as original evidence rather than overwriting them with the post-move inventory.

Shared fixture placement follows consumers; lifetime-bearing fixtures stay owned
by the test using them. The planned modules separate status/sidebar, hooks,
automation, attention and editor lifecycle from the broader suggested categories.
Learning tests already live with Learning and stay there for M2. No new abstraction,
visibility allowance, dependency/schema change or global serialization was needed.
