# Learning Mode, the failure paths

Captured from an isolated, throwaway AMF instance
(`scripts/dev/screenshot/amf-capture.sh`, scenario
`scripts/dev/screenshot/scenarios/learning-mode-errors.txt`) at `140x40`.

Every frame is a message that previously either said only what failed, or said
nothing at all. The demo project is deliberately awkward and, crucially, **not a
git repository** — so the file list comes from the fallback walk rather than
`git ls-files`, which is the path that used to drop unreadable folders in
silence. The scenario header carries the `mkdir`/`chmod` block that builds it.

| Frame | What it shows |
| --- | --- |
| `001-dashboard` | The seeded `notes-app` project and its one feature. |
| `002-first-open-help` | `K` opens the mode and the intro shows itself. Behaviour unchanged — included because the `onboarding_seen` lookup behind it used to swallow its DB error, and now logs it and falls back explicitly. |
| `003-unreadable-folder-banner` | The new banner: the walk couldn't open `vendor-cache/`, so it says how many folders are missing and points at the debug log for which. Previously they were skipped with no trace at all — a listing missing a subtree is indistinguishable from a project that hasn't got one. |
| `004-binary-file-next-step` | The binary-file message, which now ends with somewhere to go (*"Pick a source file from the list instead"*) rather than at the diagnosis. |
| `005-permission-denied-next-step` | `credentials.env` at mode 000: listable, unreadable. The case that previously logged nothing at all. |
| `006-debug-log` | `D` on the dashboard. Under the `learning` context: the folder count, **the folder named** (which the one-line banner can't carry), the `opened on` line with entry count and whether history is being saved, and both file-load failures with their paths. |

## The two defects this capture exposed

Neither would have been caught by a unit test — both are about how the output
*reads*.

**The message carried an absolute path.** On the first run, frame `005` read
`Couldn't read /tmp/claude-1000/-home-eldridger-code-.../credentials.env: …` —
a workdir prefix long enough to push the actual advice onto a fourth line, and
duplicating what the pane title already says. `load_file_lines` now takes the
repo-relative label the file list uses, guarded by an assertion in
`a_file_that_vanished_says_what_to_do_and_reaches_the_debug_log` that the
workdir prefix stays out.

**One `Enter` read the file twice.** The duplicated log line made it visible:
moving the cursor already loads the file, so `Enter` was re-reading it from disk
before shifting focus. It now only changes focus — *unless* the previous load
failed, where `Enter` is the only retry there is. That asymmetry is why
`assets/icon.bin` still appears twice in frame `006` and `credentials.env` once.
Covered by `opening_the_file_already_under_the_cursor_does_not_read_it_again`
and `opening_a_file_that_failed_to_load_tries_it_again`.
