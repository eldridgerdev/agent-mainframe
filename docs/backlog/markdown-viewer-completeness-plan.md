# Markdown viewer completeness

- **Status:** Backlog
- **Owner:** unassigned
- **Relates to:** Markdown renderer (`src/markdown.rs`), Markdown viewer
  dialog (`src/ui/dialogs/markdown.rs`)

## Why / problem

AMF's in-app Markdown viewer is good enough for headings, lists, code
blocks, and basic tables, but table headers currently disappear. The root
cause is in the table event handling: `pulldown-cmark` emits header cells
directly inside `TableHead`, while AMF only saves rows on `TableRow`.
The collected header cells are overwritten by the first body row before
rendering.

The same pass should tighten coverage around other Markdown constructs
that are either partially implemented or currently flattened in the
viewer.

## Proposed design

Fix the table state machine first, then add renderer tests around the
constructs users are likely to encounter in plans, notes, review docs,
and README excerpts. Keep the viewer terminal-native: readable,
scrollable, and stable at narrow widths matters more than pixel-perfect
GitHub rendering.

## Progress

1. [x] Render table headers.
   - Added a regression test that asserts header text is present.
   - Asserted the header/body divider is emitted.
   - Push the accumulated header row when `TagEnd::TableHead` closes.
   - Kept body-row rendering unchanged.
2. [x] Cover table alignment.
   - Added tests for left, center, and right alignment markers.
   - Preserved existing width clamping and truncation behavior.
3. [x] Cover uneven tables and empty cells.
   - Pad short rows to the detected column count.
   - Ensure empty cells remain visible rather than shifting columns.
   - Include parser alignment metadata when detecting table column count.
4. [x] Cover tables inside blockquotes and list items.
   - Verify quote/list prefixes are retained on borders and rows.
   - Check that prefixed tables still respect available width.
   - Added regression coverage for blockquote and list-item table prefixes.
5. [ ] Preserve inline styling inside table cells.
   - Keep inline code, emphasis, strong text, strike-through, and links
     styled instead of flattening each cell to a plain `String`.
   - Decide whether this needs a new table-cell span model or reuse of
     `InlineNode`.
6. [ ] Improve long table-cell behavior.
   - Decide between truncation, wrapped multi-line cells, or horizontal
     panning.
   - Prefer a simple terminal-friendly behavior that cannot corrupt table
     borders at narrow widths.
7. [ ] Make footnotes visible as footnotes.
   - Footnote parsing is enabled, but definitions need clearer rendering
     and labels.
   - Add tests for references and definitions.
8. [ ] Enable and render math deliberately.
   - `Event::InlineMath` and `Event::DisplayMath` are handled, but
     `Options::ENABLE_MATH` is not enabled.
   - Decide whether math should be plain styled text or visually marked.
9. [ ] Improve link rendering.
   - Links are styled, but destinations are hidden.
   - Decide whether to show destinations inline, in a suffix, or in a
     status/hint surface when selected.
10. [ ] Improve image fallback rendering.
    - Images currently only get styled text treatment.
    - Show alt text plus a compact source hint so Markdown docs with
      screenshots are not misleading.
11. [ ] Add a Markdown renderer fixture test.
    - Include headings, lists, task lists, tables, links, images,
      footnotes, code blocks, blockquotes, rules, and long lines.
    - Assert important text is present and raw Markdown syntax is not
      leaked where AMF claims to render it.

## Table testing fixture

Use this section as a manual smoke-test file in AMF's Markdown viewer.
The fixture tables are intentionally not fenced as code blocks, so the
viewer should render them as box-drawn grids instead of raw pipe syntax.

### Headers and divider

| Name | Status |
| --- | --- |
| AMF | Ready |

Expected:
- Header cells `Name` and `Status` are visible.
- A divider row appears between the header and body.
- Body row `AMF` / `Ready` remains visible.

### Alignment and narrow truncation

| Left | Center | Right |
| :--- | :----: | ---: |
| A | B | C |
| alphabet | bravo | charlie |

Expected:
- `A` is left-aligned, `B` is centered, and `C` is right-aligned.
- At narrow widths, long cells truncate with `…`.
- Truncation preserves table borders and column positions.

### Uneven rows

| One | Two | Three |
| --- | --- | --- |
| A | B | C |
| D | E |
| F |

Expected:
- The `D` / `E` row still has a blank third cell.
- The `F` row still has blank second and third cells.
- Later cells do not shift left when a row is short.

### Empty cells

| One | Two | Three |
| --- | --- | --- |
| A | | C |
| | B | |

Expected:
- The middle blank cell in `A | | C` remains visible.
- The first and third blank cells in `| B |` remain visible.
- Empty cells occupy grid space instead of collapsing.

### Alignment-only column count

| One | Two | Three |
| --- | --- | --- |
| A |

Expected:
- The rendered table still has three columns because the Markdown
  alignment row declares three columns.
- The missing second and third body cells render as empty cells.

### Blockquote table

> | Name | Status |
> | --- | --- |
> | AMF | Ready |

Expected:
- Every border and row keeps the blockquote gutter.
- The table still renders as a grid inside the quote.
- The grid respects the viewer width after accounting for the quote
  prefix.

### List item table

- Table:
  | Name | Status |
  | --- | --- |
  | AMF | Ready |

Expected:
- Every border and row aligns under the list item's continuation
  indentation.
- The table still renders as a grid inside the list item.
- The grid respects the viewer width after accounting for the list
  prefix.

## Open questions

- Should wide tables stay truncating, or should the viewer gain
  horizontal scroll?
- Should link destinations always be visible, or would that make compact
  project plans harder to scan?
- Is inline styling in table cells worth doing before wider Markdown
  fixture coverage, or should it remain a follow-up after the header bug?

## Reasoning / when to build

Build this when touching the Markdown viewer or when plan/review docs use
tables heavily. The table-header bug is a visible correctness issue and
should be fixed first; the rest can land incrementally as renderer
coverage improves.
