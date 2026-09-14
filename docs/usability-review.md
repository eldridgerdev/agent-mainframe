# Usability and reliability review

Reviewed the dashboard, project form and browser entry, search, help, session
pickers, harness settings, rename validation, removal flows, and their displayed
controls. Fixes are intentionally focused on existing workflows.

## Changes

- Search accepts all letters, supports arrow keys and Tab/Shift+Tab, finds
  feature nicknames, and expands the parents of a selected result. Opening an
  empty query shows the available items consistently.
- Search, notifications, commands, bookmarks, the session switcher, and saved
  session pickers keep the selected item visible. Command scrolling accounts
  for category headings as well as selectable commands.
- Markdown search accepts navigation letters and uppercase text while filtering.
- Help wraps descriptions and stores the actual scroll position, so End followed
  by Up works. Embedded-session commands explicitly require the leader key.
- Project creation supports backward field navigation, shows validation errors
  inside the form, focuses invalid fields, rejects file paths, and preserves
  input when opening the browser fails.
- Project creation and harness configuration have enough room for their fields
  at an ordinary terminal size. Project and feature deletion dialogs size to
  their wrapped content, keeping consequences and confirmation instructions visible.
- Empty session names produce visible warning toasts; whitespace-only rename
  input is rejected. Status feedback takes priority over usage meters.
- Project and session removal propagate cleanup failures before deleting their
  records. Project errors explain that earlier cleanup may already have run;
  retries tolerate worktree directories removed by an earlier attempt.
- README controls and the empty-bookmark hint match the handlers.

## Validation and limits

Validation passed: `cargo test -- --test-threads=4` (2,526 tests, including
15 new regressions), `cargo clippy --all-targets`, and `cargo fmt --check`.

Regression tests cover keyboard behavior, collapsed search targets, nicknames,
invalid project paths, cleanup failures, grouped command scrolling, status
feedback, and terminal-buffer rendering. Deletion confirmations are exercised at
80×24 and 40×20; help also has tiny-terminal crash checks. Tests use isolated
fixtures and injected managers for removal failures.

This review does not certify every external integration or terminal emulator.
Before public release, exercise first launch, authentication, a real agent
conversation, restart/resume, and cleanup with each supported harness on the
platforms being advertised. PR posting and publishing were not exercised.

Project deletion is not transactional: if cleanup of a later feature fails,
earlier features may already have stopped or lost their worktrees. The project
record now remains available and the error explains this condition.

Help and many footer hints still describe default shortcuts; workspace-specific
key remaps need a separate check. Extremely narrow terminals and unusually long
form values are not comprehensively covered across every advanced dialog.
