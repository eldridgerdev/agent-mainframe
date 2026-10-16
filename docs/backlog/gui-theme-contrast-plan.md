# GUI theme contrast and readability

- **Status:** Partial (E1–E3 landed; E4–E7 open)
- **Owner:** unassigned
- **Relates to:** `src/gui_themes.rs` (TUI → GUI theme projection),
  `gui/src/styles.css`, `gui/src/sessionSidebar.css`, `gui/src/sessions.css`,
  `gui/src/syntax.css`, `gui/src/themes.ts`, `gui/tests/darkContrast.test.ts`,
  `gui/tests/syntaxContrast.test.ts`

## Why / problem

A lot of GUI text is hard or impossible to read. On 2026-10-16 an audit ran
the debug GUI through the GUI driver. It covered 32 screens and states: every
nav panel, the project page, the feature session view, Learning (tree and file
view), Changes, Final Review, Plan interview (brief, questions and the cancel
confirmation), PR Triage, PR Review, New session, Delete feature, the TODO
list and its menus, and the feature, plan and session menus.

On each screen a DOM script computed the WCAG contrast of every visible text
node against its composited background. It also checked control boundaries
against what surrounds them. Each screen was measured under 10 themes: built-in
light, built-in dark, `amf`, `dracula`, `nord`, `catppuccin-latte`,
`catppuccin-mocha`, `gruvbox-dark`, `gruvbox-light` and
`gruvbox-material-light-medium`. A static token matrix covered all 28 catalog
themes. Thresholds are 4.5:1 for text and 3:1 for control boundaries.

The result is 73 distinct failing elements. Almost all of them trace back to
**six root causes**. The biggest one affects the theme actually in use:
`gruvbox-dark`, which the GUI follows by default.

### Root causes

| # | Cause | Where | Effect |
|---|---|---|---|
| R1 | **`surface-2` is projected from the TUI's `shortcut_background`.** That is a vivid key-hint color: yellow `#fabd2f` in gruvbox-dark, `#e5e510` in amf, orange in dracula, blue in catppuccin. The GUI uses `surface-2` as a *neutral raised surface* (24 rules). | `src/gui_themes.rs:87` | In 27/28 catalog themes these sit on a bright color block at about **1.0–1.6:1**: the selected file in Changes, Final Review and Learning; selected options in New session; modal footers (Cancel/Close text); `.badge`; `.syntax-chip`; diff and PR hunk headers; the composer head; Learning line hover and selection; and file-status `small` labels. The primary button also disappears into the footer (catppuccin 1.0:1). |
| R2 | **`text-muted` and `text-faint` are copied verbatim from TUI `text_muted`**, which is a low-contrast "comment" color in most palettes. `text-faint` is not distinct from `text-muted` either. | `src/gui_themes.rs:93-94` | Every hint, subtitle, nav label, path, ghost button, empty state, line number and debug-log timestamp is too faint. dracula 1.94, amf 2.18–2.51, nord 3.74, gruvbox-dark 4.17 (just under). This affects 86 token uses across the CSS. |
| R3 | **Status and accent colors are used as text without a contrast floor**, especially on their own 12.5%-alpha `*-soft` tints. | `src/gui_themes.rs:96-100,121-128`; light tokens `styles.css:23-53` | amber on amber-soft: gruvbox-light 1.68, catppuccin-latte 1.96, built-in light 2.94. red on red-soft: nord 2.22, amf 2.59. accent on accent-soft: gruvbox-light 2.72. These show up in chips (`tree-chip-age` "just now", `tag-accent` "In progress", status badges) and in sidebar section headings (Status/Plan in amber). |
| R4 | **Text on a filled color is fixed instead of computed.** `accent-fg` is always the theme background, `.btn-danger`/`.btn-warning`/`.todo-check` use `#fff` in light mode, and `accent-fg` is used in dark mode. | `src/gui_themes.rs:95`; `styles.css:486-487,767`; dark overrides at 1395/1445 | Delete buttons: amf 2.43, nord 3.05. Warning button: gruvbox-light 2.48, built-in light 3.73. Primary button: gruvbox-light 3.73. |
| R5 | **The built-in light palette has sub-AA tokens**, and no test guards the light palette. `darkContrast.test.ts` covers dark only. | `styles.css:23-53` | `--text-faint #8a929d` is 2.75 on the sidebar (nav sections, project paths, counts, `tree-detail`). `--amber #b07a0c` is 3.26 as text. Input, select and secondary-button borders (`--border-strong #cdd2da`) are 1.52 on white, so form fields barely show. |
| R6 | **Hard-coded colors and opacity dimming bypass the tokens entirely.** | `styles.css:311-312` (supervibe gradient with white text), 318 (`pr-merged`), 364-369 (`.kind-*` harness glyphs), 663-664 (`.term-error`), 1348-1357 (`.term-scrollback*` in light), 1365 (screenshot canvas); `sessions.css:66,72` (VS Code glyph); opacity at 378 (`ctx-stale` .75), 475 (`.btn:disabled` .55), 804 (`menu-item:disabled` .4), 860 (`tab-stopped` .55), 1164 (`learning-line-number` .5); `sessionSidebar.css:164` | Catalog themes can't restyle these. The light-mode `.kind-*` glyphs are 2.3–3.1:1 on the sidebar. Disabled primary buttons are 2.2–2.8 and disabled menu hints 1.3–1.5. |

Two problems outside the chrome:

- **The terminal in light catalog themes.** `terminalAppearance()` sets
  `minimumContrastRatio: 1` whenever the theme mode is light
  (`gui/src/themes.ts:58`). That was meant to preserve the built-in light
  terminal, which is dark. But light *catalog* themes give the terminal a light
  background, and then ANSI colors go uncorrected: gruvbox-light yellow 2.19,
  green 2.73, blue 2.80; catppuccin-latte yellow 2.31. `.term-error`
  (`#ffb4ae`) is also unreadable on a light terminal.
- **Syntax roles in catalog themes.** `syntaxContrast.test.ts` checks only the
  two built-in palettes. Projected `syn-*` roles miss 4.5:1 on the code surface
  in most catalog themes. Examples: gruvbox-light number/parameter 1.57;
  catppuccin-latte number 1.84; amf comment 2.04; dracula comment 1.68. Every
  gruvbox-material light variant lands at 2.4–3.6 for most roles.

### What you see in `gruvbox-dark` (your theme)

Worst first:

- **1.24** The selected file in Changes, Final Review and Learning, and the
  selected harness in New session: cream text on a yellow row (R1).
- **1.59** The primary button against the yellow modal footer (R1).
- **1.64** Text on that footer and on the other `surface-2` uses: the modal
  footer's Cancel/Close, the "Built-in" badges in Prompt overrides, the
  syntax chip, diff hunk headers, file status labels ("added", "modified",
  "undecided"), and composer and prompt-library hints (R1).
- **2.03** The Delete button's boundary (R4).
- **3.17** The "Quick Plan" segment and the "1 session" tree chip:
  `text-faint` on `surface-3` (R2).
- **3.37** The "Delete" item in menus: `red` on the menu surface (R3).
- **3.44–3.55** Nav counts, diff line numbers, and "In progress", "Brief" and
  "Questions" tags (R2/R3).
- **4.17** About 80 hint, subtitle, path and ghost-button texts (R2). Close to
  the threshold, but small (11–12.5px) and on every screen.

## Proposed design

Keep the token architecture. Fix it at two layers:

1. **Projection (`src/gui_themes.rs`) derives GUI roles from TUI colors
   instead of copying them.** A TUI palette describes a terminal UI where
   `shortcut_background` *should* be loud and `text_muted` *should* recede.
   The GUI needs neutral surfaces and readable secondary text. So:
   - `surface-2` = `mix(surface, text, ~6%)` and `surface-3` =
     `mix(surface, text, ~12%)`. Each is nudged until it differs visibly from
     `surface` (ΔL ≥ ~1.1:1) and still keeps `text` ≥ 7:1 and `text-muted`
     ≥ 4.5:1 on it. Stop using `shortcut_background` and `selection` for these
     roles (`selection` can stay for xterm only).
   - Separate `bg-sidebar` and `surface` slightly when `header_background ==
     background`.
   - `text-muted` = `ensure_contrast(text_muted, [bg, bg-sidebar, surface,
     surface-2, surface-3], 4.5)`, which moves the color toward `text` until
     every pair passes. `text-faint` = the same at 4.5, starting from a
     `text_muted`/background mix so that it stays a step quieter.
   - `accent`, `green`, `amber`, `red` = `ensure_contrast(color, [surfaces +
     their own soft tint over each], 4.5)`. The soft tints stay at the 0x20
     alpha.
   - **New on-color tokens** `accent-fg`, `danger-fg`, `warning-fg`,
     `success-fg`, each picked as whichever of `text`/`bg`/black/white
     contrasts most with its fill (≥ 4.5).
   - `border-strong` ensured ≥ 3:1 against `surface` and `bg`, so that
     control boundaries satisfy WCAG 1.4.11. `border` stays decorative.
   - `syn-*` roles go through `ensure_contrast` against `surface` and the
     added, removed and selected row colors, the same backgrounds
     `syntaxContrast.test.ts` uses.
   - Also project the tokens that are currently left unset: `sb-tone-*`,
     `sb-accent-issue/status/plan/usage/work/todos`, `sb-harness-pi`,
     `sb-ctx-*`, and the new `kind-*` tokens. Otherwise they inherit the
     built-in dark/light values.
   - Put the color math (`mix`, `relative_luminance`, `contrast`,
     `ensure_contrast`) in a small pure module (`src/gui_themes/color.rs`
     or at the top of `gui_themes.rs`), all unit-tested.
2. **CSS consumes only tokens.** Every literal color and every opacity
   used to dim *text* becomes a token. Fix the built-in light values. Add
   on-color and disabled tokens so a theme can't produce a low-contrast
   combination the tests don't see.

**Custom theme files** (`~/.config/amf/gui-themes/*.json`) keep the colors
they set explicitly. Derived roles they leave out get the same floor, and
explicit pairs that fail are listed in the Appearance dialog as warnings, not
errors. See the open questions.

### Suggested built-in light values

These were computed to pass on `bg`, `bg-sidebar`, `surface`, `surface-2`,
`surface-3` and the matching soft tint. Pick final values in implementation;
the tests are what enforce them.

| Token | Now | Suggested | Note |
|---|---|---|---|
| `--text-faint` | `#8a929d` (2.6–3.1) | `#687079` | ≥ 4.5 except on `surface-3`; move chips to `text-muted` |
| `--amber` | `#b07a0c` (2.9–3.7) | `#805909` | text use; keep a brighter `--amber-fill` for dots if wanted |
| `--green` | `#1f8a47` (3.4–4.4) | `#196e39` | |
| `--red` | `#cf2e2e` | `#b82929` | passes on red-soft |
| `--accent` | `#4f5bd5` | `#4954c4` | passes on accent-soft |
| new `--border-control` | (`#cdd2da` 1.52) | `#878b90` | inputs, selects, secondary buttons, switches; or retune `--border-strong` |
| `.kind-*` / VS Code glyph | `#d97757 #10a37f #6e7bff #57a143 #23a9f2` (2.3–3.1) | `#9c563f #0c775d #555fc4 #407631 #1770a0` | as `--kind-*` tokens; dark keeps the current overrides |

## Progress

### Epics and parallelization

| Epic | Priority | Needs | Can run alongside |
|---|---|---|---|
| E1 Contrast helpers + catalog regression test | P0 | — | E3, E4 |
| E2 Projection rework in `gui_themes.rs` | P0 | E1 | E3, E4 |
| E3 Built-in light palette + on-color/disabled tokens | P0 | — | E1, E2, E4 |
| E4 Tokenize hard-coded colors and opacity dimming | P1 | — (E3 for token names) | E1, E2 |
| E5 Terminal contrast in light themes | P1 | — | everything |
| E6 Live audit tooling + docs | P2 | — | everything |
| E7 Size/weight polish for small faint text | P2 | E2, E3 | E5, E6 |

E1+E2 (Rust) and E3+E4 (CSS/TS) touch disjoint files, so two people or
agents can take one track each. E2 should land before E7 because sizes are
judged on corrected colors.

### E1 — Contrast helpers + catalog regression test (P0)

- [x] Add pure color helpers: hex parse/format, alpha `over`, `mix`,
      `relative_luminance`, `contrast`, and `ensure_contrast(fg, &[bg], min)`,
      which blends toward black/white and terminates.
- [x] Unit-test the helpers: known WCAG pairs (`#777` on white = 4.48), the
      monotonic blend, and an already-passing input returned unchanged.
- [x] Add `every_catalog_theme_meets_the_gui_contrast_floor` in
      `src/gui_themes.rs` tests. Iterate every `ThemeName` and assert the
      pairing matrix: text/muted/faint on all surfaces ≥ 4.5; status colors
      on surfaces and soft tints ≥ 4.5; each `*-fg` on its fill ≥ 4.5;
      `border-strong` vs `surface`/`bg` ≥ 3; `surface-2`/`surface-3`
      distinguishable from `surface`; `syn-*` on code backgrounds ≥ 4.5.
      It should **fail on `main` today**. That proves it catches R1–R4.

### E2 — Projection rework in `src/gui_themes.rs` (P0)

- [x] Derive `surface-2`/`surface-3` from `surface`↔`text` mixes. Drop
      `shortcut_background` (`:87`) and `selection` (`:88`) for these roles.
- [x] Separate `bg-sidebar`/`surface` when the TUI gives them the same
      color, as in dracula, where `selection == header_background` and the
      selected and hovered states vanish (1.0:1).
- [x] Floor `text-muted`, then derive `text-faint` one step quieter (still
      ≥ 4.5).
- [x] Floor `accent`/`green`/`amber`/`red` against surfaces and soft tints.
- [x] Emit `accent-fg`, `danger-fg`, `warning-fg`, `success-fg` (the best of
      `text`/`bg`/black/white). Add them to `CUSTOM_TOKENS`.
- [x] Floor `border-strong` at 3:1. Emit `border-control` if E3 adds it.
- [x] Floor `syn-*` against the code, added, removed and selected
      backgrounds.
- [x] Project the missing `sb-*` tones and accents, `sb-harness-pi`,
      `sb-ctx-*` from the floored roles.
- [ ] Project `kind-*` tokens (blocked on E4 defining them in CSS).
- [x] The E1 test passes for all 28 themes. Update `entire_tui_catalog_is_projected`
      if the token set changes.
- [x] Check by eye in gruvbox-dark, dracula, catppuccin-latte and
      gruvbox-light that each theme still looks like itself (hue kept, only
      lightness moved).

### E3 — Built-in light palette + on-color/disabled tokens (P0)

- [x] Retune `:root` light tokens in `styles.css:23-53`: `--text-faint`,
      `--amber`, `--green`, `--red`, `--accent` (see the table above).
- [x] Add `--border-control` (or retune `--border-strong`) and use it for
      `input`, `select`, `textarea`, `.btn-secondary`, `.segmented` and
      `.switch`. Built-in light inputs are 1.52 today.
- [x] Add `--danger-fg`, `--warning-fg`, `--success-fg` to the light, dark
      and `[data-theme=dark]` blocks. Replace `#fff` in `.btn-danger`,
      `.btn-warning` and `.todo-completed .todo-check` (`styles.css:486-487,767`)
      and drop the per-mode overrides at `:1395` and `:1445`.
- [x] Add disabled tokens (`--disabled-fg`, `--disabled-bg`). Replace
      `.btn:disabled { opacity: .55 }` (`:475`), `.menu-item:disabled
      { opacity: .4 }` (`:804`) and `.sb-action:disabled` with colors that
      stay ≥ 3:1. Disabled is exempt from WCAG, but these are what make
      "Create feature" and "Send prompt" look broken.
- [x] Move chip text off `text-faint` on `surface-3` (`.tree-chip`
      `:286`, `.tag`, `.sb-chip`) to `text-muted` (they already used it).
- [ ] Audit the remaining `text-faint` uses that may sit on `surface-3`
      (built-in light, about 4.16:1).
- [x] Add `gui/tests/lightContrast.test.ts`, which mirrors
      `darkContrast.test.ts` for the light block: body/hint/faint text on
      every surface, status colors on soft tints, on-color buttons, and
      control borders.
- [x] Selected list rows (`.diff-file-selected`, `.learning-line-selected`)
      carry an accent edge (`inset 3px 0 var(--accent)`), matching
      `.review-line-selected`. The neutral fill alone was too subtle.
- [ ] Learning file list: highlight the open file (needs a selected flag
      from the backend view).

### E4 — Tokenize hard-coded colors and opacity dimming (P1)

- [ ] Add `--kind-claude/codex/opencode/nvim/vscode` (light + dark) and use
      them in `.kind-*` (`styles.css:364-369`), `.vscode-editor-glyph` and
      `.tree-chip-vscode` (`sessions.css:66,72`). Fold in the dark overrides
      at `styles.css:1389-1392,1439-1442` and `sessions.css:74-79`. Reuse
      the `--sb-harness-*` values where they already exist.
- [ ] Add `--purple` / `--purple-soft` for `.tree-pr.pr-merged` (`:318`).
- [ ] SuperVibe chip (`:311-312`): keep the gradient but put the label on a
      solid `--surface` pill inside it, or use a darker gradient with
      `--accent-fg`. White on the yellow stop is about 2:1.
- [ ] `.term-error` (`:663-664`): use `--red`/`--red-soft` with `--surface`
      underneath so that it works on light terminals.
- [ ] `.term-scrollback*` (`:1348-1357`): use the token form (currently
      dark-only) in light mode too, and delete the literal block.
- [ ] Replace opacity-dimmed text with colors: `.ctx-stale` (`:378`),
      `.tab-stopped` (`:860`), `.learning-line-number` (`:1164`), then drop
      their dark-only `opacity: 1` overrides.
- [ ] Add a lint-style test that fails on a new hex/rgb literal in
      `gui/src/*.css` outside token blocks (allow-list `.screenshot-canvas`
      and shadows).

### E5 — Terminal contrast in light themes (P1)

- [ ] In `terminalAppearance()` (`gui/src/themes.ts:54-60`), set
      `minimumContrastRatio` to 4.5 everywhere. If preserving exact colors on
      the built-in light-mode terminal matters, decide from the *terminal
      background's* luminance, not `theme.mode`. xterm only adjusts cells
      that fail, so passing colors don't change.
- [ ] Extend the terminal test in `darkContrast.test.ts` (or `Themes.test.tsx`)
      so that a light catalog theme gets a nonzero minimum contrast.

### E6 — Live audit tooling + docs (P2)

- [ ] Commit the audit script used for this plan as
      `scripts/dev/gui-driver/contrast-audit.js`. It walks visible text,
      composites backgrounds and opacity, reports WCAG ratios, checks control
      boundaries, and loops over themes by applying catalog tokens in place.
      Freeze transitions while measuring; otherwise `getComputedStyle` reads
      mid-transition colors.
- [ ] Add an `audit-contrast [--scope] [--themes]` driver subcommand, or
      document the `eval` recipe in `docs/development/gui-ai-driver.md`.
- [ ] Note in `themes/README.md` / the custom-theme docs that the GUI floors
      derived roles and warns about explicit low-contrast pairs.

### E7 — Size/weight polish for small faint text (P2)

- [ ] Raise the floor of 10.5px text (`.tree-chip`, `.tree-detail`,
      `.tree-context`, `.tree-chip-detail` at 10px) to 11px. Raise 11px
      section labels (`.nav-section`, `.sb-section-head h3`, `.syntax-chip`)
      to 11.5px, or give them weight 600.
- [ ] Re-run the E6 audit across all 28 themes and both built-ins, with zero
      non-disabled failures.

## Verification

- `cargo test gui_themes` passes, including the new catalog floor test.
- `npm --prefix gui test` passes, including the new light contrast test and
  the extended syntax and terminal tests.
- Run the E6 audit in a GUI driver session over the same 32 screens. No
  non-disabled text below 4.5:1 and no control below 3:1, in every theme.
- Visual check (screenshots on request): Changes with a selected file, a
  modal footer, Prompt overrides badges, and the session view with sidebar,
  in gruvbox-dark, dracula, catppuccin-latte and built-in light.

## Open questions

- **Custom theme files:** should explicit custom tokens be auto-corrected,
  only warned about, or both, with a `"strict": false` opt-out? The proposal
  warns and leaves them alone.
- **Built-in light terminal:** should it stay dark (the current deliberate
  choice), or follow the light page now that `minimumContrastRatio` handles
  readability?
- **AAA for body text:** the floor is AA (4.5). Some users asking for
  "readable" want 7:1 for `text`. That is easy to add for `text` only.
- **`text-faint` vs `text-muted` in light mode:** at AA they end up close
  (`#687079` vs `#5a6370`). Either accept that, or drop `text-faint` from
  real text and keep it for decorative glyphs only.

## Reasoning / when to build

Build E1–E3 now. The default GUI theme follows the TUI, and with the TUI on
`gruvbox-dark` the selected file in every diff and review list is 1.24:1, so
core review flows are hard to use. The fix is small and central: one
projection function plus one CSS token block. The tests make it hard to
regress. E4–E5 are cleanup with a clear scope, and E6–E7 are for
maintainability and polish.
