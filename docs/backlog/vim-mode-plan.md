# Vim mode

- **Status:** Partial — Tier 1 core editing largely shipped (v0.24.0:
  undo/redo, operator+motion, delete/yank/paste, `dd`/`yy`/`D`/`e`);
  `c`/`cc`/`C` and Tiers 2-3 remain.
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
- **Count accumulator** — collect leading digits and apply a repeat
  count to motions/operators.

## Backlog (ranked by importance)

### Tier 1 — Core editing (highest value)

- [x] **Undo / redo** — `u`, `Ctrl-r`. Introduces the undo stack used
  by everything below. (Foundational)
- [x] **Operator + motion framework** — operator-pending state so
  `d`/`c`/`y` combine with any motion. (Foundational)
- [x] **Delete operator** `d` with motions — `dw`, `db`, `de`, `d$`,
  `d0`, `dh`, `dl`, `dj`, `dk`.
- [ ] **Change operator** `c` with motions — `cw`, `cb`, `c$`, etc.
  (delete + enter insert).
- [x] **Yank operator** `y` with motions — `yw`, `y$`, `ye`, etc.
  Introduces register storage. (Foundational)
- [x] **Paste** — `p` (after cursor) and `P` (before), charwise and
  linewise aware. Delete/`x` also populate the register (`ddp`, `xp`).
- [x] **Delete line** `dd` (linewise).
- [x] **Yank line** — `yy` / `Y` (linewise).
- [ ] **Linewise change** — `cc` (and `S` as `cc`).
- [x] **Delete to line end** `D` (= `d$`).
- [ ] **Change to line end** `C` (= `c$`).
- [x] **Word-end motion** `e` — standalone and as a `d`/`c`/`y` target
  (`de`, `ce`).

### Tier 2 — Everyday productivity

- [ ] **Counts** — numeric prefixes (`3w`, `5j`, `2dd`, `d3w`).
  (Foundational accumulator)
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

## Notes

- Surfaces in scope today: the Compose box and the Steering Prompt.
  Reach to other inputs (feature-creation fields, search query) is a
  separate decision and not tracked here.
- Each feature should land with unit tests in the `tests` module of
  `src/editor.rs`, mirroring the existing test style.
- Cursor is a byte offset over a `String`; keep all new motions
  char-boundary safe via the existing `prev_boundary` / `next_boundary`
  helpers.
