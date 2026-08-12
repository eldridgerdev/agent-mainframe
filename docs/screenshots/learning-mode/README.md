# Learning Mode, end to end

Captured from an isolated, throwaway AMF instance
(`scripts/dev/screenshot/amf-capture.sh`, scenario
`scripts/dev/screenshot/scenarios/learning-mode.txt`) at `160x44`, against a
scratch `taskline` repo seeded with a `README.md`, `CLAUDE.md`, `Cargo.toml`
and `src/main.rs` so the pinned **Start here** group has real files to find.

Frames 009–011 span one real headless `claude` run — the answer is a live
call, not a fixture.

| Frame | What it shows |
| --- | --- |
| `001-dashboard` | The seeded project. Learning Mode creates no session, so it appears nowhere in the tree. |
| `002-first-open-help` | `K` opens the mode; on a project's first visit the help shows itself unprompted, leading with "nothing here changes your files" and "no question is too basic" before any key. |
| `003-overlay-start-here` | The three panes, the permanent `read-only` header marker, and the pinned orientation group. The cursor opens on *Tour this whole project*, not a group header. |
| `004-starter-questions` | `t`, filtered to the anchor — only the project-level presets while the project is selected. |
| `005-question-prompt-project` | The preset loaded as editable text (nothing asked yet), with the anchor spelled out: `About: this whole project`. |
| `006-file-content` | `src/main.rs` with line numbers and `crate::highlight` syntax. |
| `007-line-range-selected` | `v` starts a range; the anchor line reads `lines 11-15 of src/main.rs`. |
| `008-question-typed` | `e` — "explain this to me", the teaching intent. |
| `009-asking-in-flight` | Non-blocking: the row reads `thinking…` and the header counts `1 answer still generating`. |
| `010-answered` | The run lands; the row flips to `answered` and the counter clears. |
| `011-answer-markdown` | The answer as rendered markdown — headings, a fenced Rust block, inline code — with the newcomer template defining `ExitCode`, `Result` and `Vec<String>` before using them. |
| `012-follow-up-prompt` | `F` keeps the parent's place (`lines 11-15`, though the file list moved since) and says so. |

## Known nits visible in `011`

Both belong to the shared markdown renderer, not to Learning Mode:

- Inline code at the very start of a list item is dropped — `` `Ok(())` — it
  worked`` renders as `• — it worked`.
- The answer pane's metadata line ends with the status word twice
  ("…written for a newcomer reader · answered").
