# Syntax Highlighting Research Plan

Date: March 9, 2026

## Decision

Selected backend: `tree-sitter-highlight`

Reason for final selection:

- better long-term fit for reusable code-aware rendering
- cleaner path to richer future features than a regex/token-based highlighter
- portable if we keep AMF ownership of the render model and backend boundary

## Context

AMF renders most custom UI content as `ratatui::text::Line` and `Span`, with theming owned by `src/theme.rs`. The new diff viewer already builds its own wrapped, line-numbered, background-aware rows in `src/ui/dialogs/diff.rs`, so syntax highlighting needs to fit into that pipeline instead of replacing it.

This decision should be reusable beyond the diff viewer:

- branch/file diff views
- file preview dialogs
- markdown/code-block rendering
- prompt/history viewers
- any future code-inspection surface

## What We Need

- good language coverage out of the box
- portable integration that does not lock us to one screen
- direct conversion into our own render model, not just ANSI strings
- plain-text fallback when language detection fails
- control over background merging so diff add/remove bands still win
- acceptable performance for full-file and per-file diff rendering
- low operational complexity for packaging and cross-platform builds

## Chosen Option

### 1. `tree-sitter-highlight` with an AMF-owned highlighting layer

This is the selected implementation path.

Why:

- syntax categories are semantic and stable enough to map into an AMF-owned code theme
- better fit if we later want selections, symbols, semantic navigation, or editor-like features
- portable as long as AMF owns the API, style model, caching, and merge rules
- good match for future reuse outside the diff viewer

Tradeoffs we are accepting:

- more setup than `syntect`
- we need to manage language crates and highlight query configuration
- language coverage must be built intentionally instead of getting a large default bundle for free

## Second-Best Option

### 2. `syntect` behind the same AMF interface

This remains the best fallback if `tree-sitter` proves too heavy in practice.

Why it is still attractive:

- simpler initial setup
- broad language coverage immediately
- easy line-oriented integration

Why it is not the chosen path now:

- weaker foundation for future code-aware features
- less aligned with a long-term reusable code intelligence layer

## Options I Considered But Do Not Recommend As The Base

### 3. `tui-syntax-highlight` or `syntect-tui`

Good for prototypes, not ideal as the core architecture.

Why not as the foundation:

- they are ratatui adapters, not AMF-level highlighter architecture
- they hand back already-renderable text, but AMF still needs custom row composition and background overrides
- using them directly would make later reuse outside simple code blocks more awkward

Use them only as reference code or for a short spike.

### 4. `bat` subprocess / ANSI output

Not recommended.

Why not:

- subprocess coupling
- ANSI parsing adds another translation step
- harder to control portability and caching
- harder to merge syntax colors with diff backgrounds and custom gutters cleanly

## Recommended Architecture

Create a reusable highlighter module instead of embedding crate calls in the diff UI.

Suggested structure:

```text
src/highlight/
├── mod.rs
├── service.rs      # public API used by UI code
├── detect.rs       # infer language from path / extension / shebang
├── model.rs        # AMF-owned highlighted line/span types
├── tree_sitter.rs  # first backend
└── theme.rs        # code-theme mapping and style merge rules
```

### Public API shape

The public API should return AMF-owned display data, not raw `syntect` or `ratatui` types.

Example shape:

```rust
pub struct HighlightRequest<'a> {
    pub path: Option<&'a Path>,
    pub language_hint: Option<&'a str>,
    pub source: &'a str,
}

pub struct HighlightedText {
    pub language_name: Option<String>,
    pub lines: Vec<HighlightedLine>,
}

pub struct HighlightedLine {
    pub spans: Vec<HighlightedSpan>,
}

pub struct HighlightedSpan {
    pub text: String,
    pub style: HighlightStyle,
}
```

`HighlightStyle` should be AMF-owned and renderer-neutral enough that we can translate it into `ratatui::Style` today and something else later if needed.

### Important rule: syntax foreground, AMF background

For diffs, syntax highlighting should mostly control foreground and text modifiers.
Diff rendering should keep ownership of background colors and special fills.

That merge rule lets us do all of the following cleanly:

- keep red/green diff bands
- keep slash-filled missing panes
- preserve syntax token colors on top of those backgrounds
- reuse the same highlighter in non-diff surfaces with normal backgrounds

### Theme strategy

Do not try to force syntax coloring into the existing AMF UI theme fields.

Instead:

- keep the app theme for chrome, borders, dialogs, status, and diff backgrounds
- add a separate code-theme choice for syntax token palettes
- start with one dark code theme and one light code theme
- later map AMF themes to preferred code themes if we want automatic pairing

This separation will make the highlighter much easier to reuse across views.

### Caching strategy

Start simple but leave room for growth:

- keep one lazily initialized syntax set for the whole process
- detect syntax by file path first, then fallback to plain text
- cache highlighted output by `(path/language hint, source hash, code theme)`
- invalidate on content change

For the diff viewer, caching by file patch content is enough.

With tree-sitter, also cache compiled `HighlightConfiguration` values per language and share them process-wide.

## Recommended Rollout

### Phase 1: portable base

- add `src/highlight/` with an AMF-owned model and trait/service boundary
- implement `tree-sitter-highlight` backend first
- start with a deliberately small language set that matches AMF usage well:
  - Rust
  - Markdown
  - TOML
  - JSON
  - YAML
  - Shell
- add plain-text fallback
- add unit tests for language detection and style merging
- add tests for unknown-language fallback and missing-query handling

### Phase 2: diff viewer integration

- highlight the selected file in unified mode first
- preserve current diff backgrounds and line-number gutters
- then apply the same engine to side-by-side rows
- add snapshot tests for wrapped lines and add/remove panes

### Phase 3: broader reuse

- use the same highlighter in any file preview dialog
- use it for markdown fenced code blocks if AMF gains richer markdown rendering
- add config for code theme selection

### Phase 4: language expansion and optional fallback backend

- expand tree-sitter language coverage based on real usage
- if startup size or maintenance cost becomes a problem, add `syntect` as a fallback backend behind the same AMF interface
- keep UI surfaces unchanged

## Recommendation Summary

Recommended now:

- `tree-sitter-highlight` core engine
- AMF-owned highlighting service
- AMF-owned output model
- explicit style-merge rules for diff backgrounds

Recommended later if needed:

- `syntect` as a fallback backend under the same interface

Not recommended as the main foundation:

- `tui-syntax-highlight`
- `syntect-tui`
- `bat` subprocess output

## Why This Is The Best Fit For AMF

This gives us the best balance of:

- portability across UI surfaces
- stronger future code-awareness
- enough control for custom diff rendering
- a clean extension path as more code-centric views are added

## Primary Sources

- `syntect` repository and README: https://github.com/trishume/syntect
- `syntect::parsing::SyntaxSet` docs: https://docs.rs/syntect/latest/syntect/parsing/struct.SyntaxSet.html
- `syntect::easy::HighlightLines` docs: https://docs.rs/syntect/latest/syntect/easy/struct.HighlightLines.html
- `syntect::highlighting::ThemeSet` docs: https://docs.rs/syntect/latest/syntect/highlighting/struct.ThemeSet.html
- `tree-sitter` syntax highlighting docs: https://tree-sitter.github.io/tree-sitter/3-syntax-highlighting.html
- `tree-sitter-highlight` docs: https://docs.rs/tree-sitter-highlight/latest/tree_sitter_highlight/
- `HighlightConfiguration` docs: https://docs.rs/tree-sitter-highlight/latest/tree_sitter_highlight/struct.HighlightConfiguration.html
- `syntect-tui` docs: https://docs.rs/syntect-tui
- `tui-syntax-highlight` docs: https://docs.rs/tui-syntax-highlight
- `bat` docs: https://docs.rs/crate/bat/latest
