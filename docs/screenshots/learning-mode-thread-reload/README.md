# A follow-up stays with its question across a reopen

Captured from an isolated, throwaway AMF instance
(`scripts/dev/screenshot/amf-capture.sh`, scenario
`scripts/dev/screenshot/scenarios/learning-mode-thread-reload.txt`) at
`160x44`, against a scratch `taskline` repo seeded with a `README.md`,
`CLAUDE.md`, `Cargo.toml` and `src/main.rs` so the pinned **Start here**
group has real files to find.

All three answers are real headless `claude` runs, not fixtures.

| Frame | What it shows |
| --- | --- |
| `001-overlay-start-here` | `K` opens the mode on the seeded feature: three panes, the permanent `read-only` header marker, and the pinned orientation group. |
| `002-range-selected` | `v` starts a range over `load()` in `src/main.rs`; the anchor is spelled out above the panes. |
| `003-threaded-live` | Three answered questions with the follow-up threaded under its parent — the live list, which always worked. |
| `004-threaded-after-reopen` | The same history after `q` closed the overlay and `K` reopened it: the follow-up is still under *"What does this function do?"*, and the unrelated question keeps its place below the thread. |

## The defect this capture exposed

`before-follow-up-under-the-wrong-question.png` is the pre-fix version of
frame `004`. Both frames render the **same scratch database** — the before
one was captured by re-opening that database with a binary built without
the fix, so no extra questions were asked for it. The only difference
between the two images is the row order.

**A reopened history lost its threading.** Rows reload
`ORDER BY created_at`, but a follow-up is asked *after* whatever else was
asked in between — and the history pane takes a row's *placement* from the
list while taking only its *indentation* from `parent_qa_id`. So
*"What happens if that file does not exist yet?"* came back indented under
*"What is stored in tasks.txt"*, a question it has nothing to do with.

`thread_rows` now reorders a loaded history through `thread_insert_index`,
the same function that positions a brand-new follow-up, so there is one
notion of order rather than two that agree until the overlay is closed.
Deep dives thread by `parent_qa_id` too, so they came along with it; a row
whose parent is missing is kept rather than dropped, since it is the only
copy of a question someone asked.

The order the scenario drives is the only one that shows this: a follow-up
on the *most recent* question is adjacent to its parent either way.
