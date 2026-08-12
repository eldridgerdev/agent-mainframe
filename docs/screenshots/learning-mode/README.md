# Learning Mode, end to end

Captured from an isolated, throwaway AMF instance
(`scripts/dev/screenshot/amf-capture.sh`, scenario
`scripts/dev/screenshot/scenarios/learning-mode.txt`) at `160x44`, against a
scratch `taskline` repo seeded with a `README.md`, `CLAUDE.md`, `Cargo.toml`
and `src/main.rs` so the pinned **Start here** group has real files to find.

Frames `009`–`011` span one real headless `claude` run, and `013` starts a
real interactive session — the answers are live calls, not fixtures.

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
| `011-answer-markdown` | The answer as rendered markdown, and the regression shot for the fix below — every bullet keeps the inline code span it opens with, and the provenance line states the status once. |
| `012-follow-up-prompt` | `F` keeps the parent's place (`lines 11-15`, though the file list moved since) and says so. |
| `013-escalate-composer` | `S` hands the answer to a live agent: a real session titled `Learning: src/main.rs:11-15`, its composer pre-filled with the anchor, question and answer and **not** sent (`Enter send` still offered), closing on the line that says this is the one place the read-only promise ends. Claude's own first-run "trust this folder" gate is visible behind it. |

## The two defects this capture exposed

`before-inline-code-dropped.png` is the pre-fix version of frame `011`, kept
because reading the rendered answer — rather than the code that produces
it — is what surfaced both, and neither would have been caught by a unit test
written against the same assumptions as the bug.

- **The shared markdown renderer dropped whatever opened a list item.**
  `` `Ok(())` — it worked`` rendered as `• — it worked`.
  `MarkdownRenderer::push_inline_text` only appended when a text block was
  already open, and a *tight* list item carries its content with no
  `Tag::Paragraph` around it, so the first inline event had nowhere to land.
  Task-list checkboxes (always first in their item) and footnote bodies
  opening with inline code went the same way. The block is now opened at that
  single point. The fix is shared with every markdown surface in AMF.
- **The answer pane stated its status twice** — "answered by Claude · … ·
  answered" — and said "answered by" of a row that had not answered. The
  status now rides the opening verb alone: `Claude is answering`,
  `queued for Claude`, `Claude couldn't answer`.
