# Vim mode

- **Status:** Partial — Tier 1 core editing complete (undo/redo,
  operator+motion, delete/yank/change/paste, `dd`/`yy`/`cc`/`D`/`C`/`e`).
  Tier 2 started: **counts** (`3w`, `2dd`, `d3w`, `2d3w`) shipped; visual
  mode, text objects, find/till, and the rest of Tiers 2-3 remain.
- **Owner:** unassigned
- **Relates to:** `src/editor.rs` (`TextEditor`),
  `src/ui/dialogs/editor_view.rs`; shipped vim work in v0.24.0

A living, ranked checklist of vim features we want in our in-house
editor. We extend `src/editor.rs` (`TextEditor`) rather than adopt a
third-party crate, so this file is the source of truth for what is done
and what is next. Check items off as they ship (see **Backlog** below).

## Approach

- Build on the existing hand-rolled `TextEditor` in `src/editor.rs`.
- No new dependencies; keep the current rendering path
  (`src/ui/dialogs/editor_view.rs`) and the Compose / Steering Prompt
  wiring intact.
- Implement features one at a time, top of the list first, committing
  this file as we go so progress is tracked.

## Current State (already supported)

These already work and are NOT part of the backlog below:

- Mode toggle Plain ↔ Vim; Insert ↔ Normal via `Esc`.
- Enter insert: `i`, `a`, `A`, `I`, `o`, `O`.
- Motions: `h` `l` `j` `k` (+ arrow keys), `0`, `$`, `w`, `b`.
- Edit: `x` (delete char under cursor).

## Architecture Gaps (foundational prerequisites)

Several backlog items share infrastructure that does not exist yet.
Building these first unblocks whole groups of features, so they are
woven into the ranking below rather than listed separately:

- **Undo/redo stack** — _(done)_ the buffer was a raw `String` with no
  history; now backs `u` / `Ctrl-r` and makes every edit operator safer.
- **Register storage** — _(done)_ unnamed register backing yank/paste;
  named registers remain a Tier 3 feature.
- **Operator-pending state** — _(done)_ a small state machine so an
  operator (`d`/`c`/`y`) can wait for a motion or text object.
- **Count accumulator** — _(done)_ leading digits collect into a repeat
  count applied to motions and operators (`3w`, `2dd`, `d3w`, and
  multiplied counts like `2d3w`).

## Backlog (ranked by importance)

### Tier 1 — Core editing (highest value)

- [x] **Undo / redo** — `u`, `Ctrl-r`. Introduces the undo stack used
  by everything below. (Foundational)
- [x] **Operator + motion framework** — operator-pending state so
  `d`/`c`/`y` combine with any motion. (Foundational)
- [x] **Delete operator** `d` with motions — `dw`, `db`, `de`, `d$`,
  `d0`, `dh`, `dl`, `dj`, `dk`.
- [x] **Change operator** `c` with motions — `cw` (acts like `ce`),
  `cb`, `c$`, `c0`, `ch`, `cl`, `ce`, etc. (delete + enter insert as one
  undo step).
- [x] **Yank operator** `y` with motions — `yw`, `y$`, `ye`, etc.
  Introduces register storage. (Foundational)
- [x] **Paste** — `p` (after cursor) and `P` (before), charwise and
  linewise aware. Delete/`x` also populate the register (`ddp`, `xp`).
- [x] **Delete line** `dd` (linewise).
- [x] **Yank line** — `yy` / `Y` (linewise).
- [x] **Linewise change** — `cc` (and `S` as `cc`). Keeps an empty line
  in place (unlike `dd`) and enters insert.
- [x] **Delete to line end** `D` (= `d$`).
- [x] **Change to line end** `C` (= `c$`).
- [x] **Word-end motion** `e` — standalone and as a `d`/`c`/`y` target
  (`de`, `ce`).

### Tier 2 — Everyday productivity

- [x] **Counts** — numeric prefixes (`3w`, `5j`, `2dd`, `d3w`), with
  operator/motion counts multiplying (`2d3w`). `0` stays the start-of-line
  motion unless a count is already in progress. (Foundational accumulator)
- [ ] **Visual mode** `v` and linewise `V` — selection + operate
  (`d`/`c`/`y` on the selection).
- [ ] **Text objects** — `iw`, `aw`, then quote/bracket pairs `i"`,
  `i'`, `i(` / `ib`, `i{` / `iB`, `i[`, and their `a` variants; usable
  with operators (`ciw`, `di"`) and visual (`viw`).
- [ ] **Find / till** — `f`, `F`, `t`, `T`, plus `;` / `,` to repeat;
  usable as operator targets (`df,`, `ct)`).
- [ ] **First non-blank motion** `^` as a real motion (and `d^`).
- [ ] **Document motions** — `gg` (top) and `G` (bottom), count-aware
  (`5G`).
- [ ] **Replace char** `r<char>`; substitute char `s` (= `cl`).
- [ ] **Join lines** `J`.
- [ ] **Toggle case** `~`.

### Tier 3 — Power features

- [ ] **Dot-repeat** `.` — replay the last change. Requires recording a
  structured "last change" alongside edits.
- [ ] **Search** — `/`, `?`, `n`, `N`; usable as operator targets.
- [ ] **Named registers** — `"a`-prefixed yank/paste and the unnamed
  register semantics; expands register storage from Tier 1.
- [ ] **Bracket match motion** `%`.
- [ ] **Paragraph motions** `{` and `}`.
- [ ] **WORD motions** — `W`, `B`, `E` (whitespace-delimited).
- [ ] **Half-page scroll** — `Ctrl-d`, `Ctrl-u` (integrates with the
  dialog scroll-offset logic).
- [ ] **Marks** — `m<char>` and jump `` `<char> `` / `'<char>`.
- [ ] **Macros** — `q<reg>` record, `@<reg>` / `@@` replay.

### Editor behavior & persistence

Not editing features, but related vim-mode polish:

- [ ] **Persist the Plain ↔ Vim toggle.** Today the compose editor's vim
  mode is chosen per session and resets each launch. Save whether vim is
  enabled so the choice survives restarts — store it as a user preference in
  `AppConfig` (`config.json`), like other persisted defaults, and seed new
  `TextEditor` instances (compose, steering, and any future prompt-library
  editor) from it on creation. Toggling vim at runtime should update the
  persisted value so the next session starts in the same mode.

## Notes

- Surfaces in scope today: the Compose box and the Steering Prompt.
  Reach to other inputs (feature-creation fields, search query) is a
  separate decision and not tracked here.
- Each feature should land with unit tests in the `tests` module of
  `src/editor.rs`, mirroring the existing test style.
- Cursor is a byte offset over a `String`; keep all new motions
  char-boundary safe via the existing `prev_boundary` / `next_boundary`
  helpers.
