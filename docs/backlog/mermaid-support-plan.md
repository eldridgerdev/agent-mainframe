# Mermaid support: investigation and proposed plan

- Status: Terminal MVP implemented and locally validated. Later phases remain proposed.
- Investigated: 2026-10-10.
- Scope: Mermaid diagrams embedded in AMF-owned Markdown documents and answers.
- Recommendation: Terminal-first text diagrams, followed by desktop SVG rendering.

## MVP implementation update

The accepted MVP covers the terminal Markdown viewer/current plan and the plan
interview review gate. It adds a document-level `d` diagram/source toggle and
uses the existing file refresh action. Other Markdown surfaces, desktop SVG,
prompt changes and graphics protocols remain deferred.

An isolated renderer evaluation compiled Merman 0.8.0 with only ASCII,
flowchart and sequence features and exercised small charts successfully.
Integration exposed a Cargo conflict: AMF's Ratatui 0.29 pins
`unicode-width =0.2.0`, while Merman's ASCII dependency requires `^0.2.2`.
Mermansi shares that conflict. Upgrading the TUI stack would broaden this MVP.

The implementation instead pins [Selkie](https://github.com/btucker/selkie)
(`selkie-rs =0.3.0`, MIT) with default features disabled. It resolves with AMF's
existing dependencies and adds 13 lockfile
packages without upgrading existing ones. Isolated release runs rendered small
flowcharts and sequences in approximately 0.1–2.7 ms; these are smoke measurements,
not large-chart performance guarantees. The upstream terminal path omits some
sequence constructs and measures labels by character count, so AMF refuses
those constructs and wide/combining text. Every semantic label must survive
rendering, and overflow uses source fallback without truncation.

One App-owned worker runs layout outside drawing, with a bounded queue and
per-document reply channels. Small source/model/canvas limits contain work;
cancellation applies between stages, not within Selkie's layout. Source and
available width identify results, including cached failures. Refresh discards
artifacts. No database or authored Markdown format changes are needed.

The [MVP usage guide and examples](../mermaid-mvp.md) describe the actual
supported subset. The original investigation and later-stage proposal follow.

Initial MVP validation on 2026-10-10 passed `cargo build --locked`,
`cargo test --workspace --locked` (3,470 library tests and 7 desktop tests),
`cargo fmt --check`, and strict all-target workspace Clippy. Thirteen added
tests cover rendering, source fallback, width changes, fencing/prefixes,
cancellation/stale replies, the documented examples, and viewer/plan-review
integration. The existing live-GitHub acceptance test remains ignored by its
pre-existing annotation. No screenshot capture was requested or performed.

The follow-up showcase adds grouped flowchart phases with decisions and retry
loops, plus a four-participant sequence with a refinement loop and success/failure
branches. Visual validation was explicitly requested for these examples. It
exposed overlapping decision borders from the browser-oriented size estimator;
layout now reserves the text shapes' actual cell dimensions. The screenshot
renderer also uses a Braille-capable fallback font for connector glyphs.

## Feasibility and recommendation

Mermaid support is feasible. AMF already owns the Markdown readers where it
would be useful. The terminal and desktop need different presentation paths:

- The terminal can display a Mermaid diagram as Unicode boxes, labels and
  connectors using a native Rust renderer. This fits Ratatui's cell model and
  does not require a graphics-capable terminal, Node or Chromium. It is a
  terminal interpretation of Mermaid, with a deliberately bounded syntax
  contract, rather than graphical parity with Mermaid's browser renderer.
- The React/Tauri desktop can use the official Mermaid JavaScript package to
  render SVG. Bundle its assets locally and render inside AMF's document panes.
- Pixel graphics inside the terminal are possible through image protocols, but
  add terminal detection, tmux compatibility, image lifetime and viewport work.
  Treat that as a later enhancement if text diagrams prove insufficient.

Start with existing Markdown content, not a new chart dashboard or diagram
editor. Keep Mermaid source in the existing document/answer storage. Derived
renderings are disposable; this proposal needs no database migration.

This investigation inspected AMF source, upstream documentation, dependency
manifests and the published Merman package. It did not compile a renderer,
prototype integration, measure performance, or capture visual evidence.

## Where this fits in AMF

| Surface | Current implementation | Proposed use |
| --- | --- | --- |
| Markdown file viewer and current plan | `src/markdown.rs`, `src/ui/dialogs/markdown.rs`, `src/app/plan.rs` | Read architecture, control flow and dependencies inline in existing `.md` files. |
| Plan interview review and critique | `src/ui/dialogs/plan_interview.rs` uses `draw_markdown_document` | Inspect a proposed design before accepting the plan; raw editing preserves the Mermaid source. |
| Learning answers | `src/ui/dialogs/learning.rs` uses the same document helper | Ask for an explanation of a call path or state machine and read its diagram beside the explanation. |
| PR triage and Final Review explanations | `src/ui/dialogs/pr_review.rs`, `diff.rs`, `ai_review.rs` call `render_markdown` | Understand Mermaid blocks in PR comments, investigations, walkthroughs and overview text. |
| Desktop documents | `gui/src/Markdown.tsx` | One diagram component serves plans, Learning answers, investigations and review prose. |
| Desktop PR content | `gui/src/PrMarkdown.tsx` | Add the same diagram component without disturbing its sanitized HTML and native image/link adapters. |

The TUI parser already retains fenced-code language and body in
`CodeBlockState`. `finish_code_block` currently renders every language as a
framed code listing. This is the integration boundary for prepared diagram
output; it is not a place to launch a renderer process or perform layout work.

The desktop has two Markdown implementations. Its basic `Markdown.tsx` scans
lines and discards fence language; `PrMarkdown.tsx` already uses
`react-markdown`, GFM and sanitization. Mermaid detection needs real fenced-code
metadata. Prefer using the existing Markdown parser dependency for the basic
component as well, preserving its current safe treatment of raw HTML and
verifying current behavior. Preserve the PR component's separate HTML/image
policy rather than merging policies as an incidental part of this work.

AMF's agent pane is a separate boundary: it captures tmux's ANSI screen and
reconstructs terminal cells. Rendering Mermaid in AMF documents will not turn
arbitrary agent scrollback into a structured Markdown conversation or preserve
images emitted by a child harness. Users would open the generated plan/doc in
AMF's reader. Reinterpreting live terminal output is outside this proposal.

Remote Control currently exposes terminal/status/diff/TODO workflows; adding a
new diagram/document reader there is a separate product change.

## Rendering options investigated

| Approach | Advantages | Limitations | Recommendation |
| --- | --- | --- | --- |
| Merman Rust facade with ASCII feature | Native text output; explicit width/overflow policy; cancellation and resource controls; optional SVG path later | Independent Mermaid implementation; partial terminal semantics; recent release/API changes | First candidate for TUI evaluation. |
| mermansi | Native terminal output with broader claimed family coverage | Version 0.1.6 pins Merman alpha.3 internals; another compatibility layer | Backup candidate if Merman fails required examples. |
| mermaid-rs-renderer (`mmdr`) | Native SVG/PNG without browser processes | Image output still needs a terminal display solution; does not solve AMF's cell-based Markdown path alone | Alternative for a future native graphics path. |
| Official Mermaid JavaScript | Upstream SVG rendering and broad syntax support in a browser/webview | Adds a frontend dependency; requires WebView/font/CSP validation | Preferred desktop path. |
| Official Mermaid CLI (`mmdc`) | Official renderer; SVG/PNG/PDF export | Node and Puppeteer/browser deployment; process ownership and startup overhead | Optional export tool later, not a runtime prerequisite. |
| Native SVG/PNG plus `ratatui-image` | Graphical diagrams in supporting terminals | Terminal/tmux matrix, raster readability, image clipping/cleanup, dependency compatibility | Defer from initial delivery. |

Merman 0.8.0 is published on crates.io (2026-10-05) and requires Rust 1.95.
The workspace's installed compiler is 1.99.0, so that minimum does not block
this checkout. Its published manifest and downloaded archive were checked;
some README/embedding documentation still says the release is unpublished.
Use a pinned published version and matching API/source rather than mixing
examples from alpha releases and `main`.

Disable Merman's defaults: they select all diagrams and the complete SVG/ELK
path, which are unnecessary for the initial terminal feature. Evaluate only
ASCII output plus the chosen diagram families. Both Merman and mermansi declare
MIT OR Apache-2.0 licensing; inspect the actual selected transitive dependencies
and build impact during the evaluation.

Merman's tagged terminal documentation distinguishes diagrammatic output
(including Flowchart, Sequence, State, Class, ER and XYChart) from structured
reports (including Gantt, GitGraph and Mindmap). It describes semantic coverage
as partial. Do not advertise every parsed family as a supported terminal chart.

`ratatui-image` supports Kitty, Sixel and iTerm2 with a half-block fallback.
That fallback approximates image pixels and is a poor default for small text
labels. Its current development manifest uses Ratatui 0.30.1; AMF uses 0.29.
A graphics phase must select a compatible release or separately justify a
Ratatui upgrade, rather than taking the latest dependency automatically.

## Proposed user experience

1. An agent or user writes an ordinary fenced `mermaid` block in a plan,
   repository document or answer. Only explicitly labelled blocks render as
   diagrams; ordinary code blocks keep their current behavior.
2. Opening the existing Markdown reader shows the diagram inline in a labelled
   frame. The initial supported families are Flowchart (`flowchart`/`graph`) and
   Sequence (`sequenceDiagram`). State (`stateDiagram-v2`) is the next candidate
   if the evaluation demonstrates acceptable support.
3. Existing vertical scroll and navigation continue to work. Diagram connector
   rows are never passed through prose word wrapping. Narrow windows use a
   verified compact layout where possible; if the result cannot fit, show a
   width message and the source rather than clipping away relationships.
4. A document-level diagram/source toggle makes the authored text available.
   Use a key that is unused in the particular TUI view; keep existing shortcuts.
   In the desktop, each diagram gets a Show source control and an enlarged view
   with zoom/pan and a fit/reset action.
5. Invalid syntax, unsupported features and resource refusals show a compact
   explanation alongside the original source. A bad diagram never hides the
   surrounding document or prevents plan review/acceptance.
6. The existing refresh action reloads edited files and invalidates prepared
   results. Live filesystem watching and graphical editing are later work.

The desktop would support the families accepted by its pinned official Mermaid
release, while the TUI initially supports the small advertised subset. Both
interfaces retain source access. Their layouts need not match, and the UI must
not imply that a terminal approximation is the full graphical chart.

## Implementation sequence after acceptance

### 1. Evaluate the renderer before integrating it

Use an isolated throwaway consumer crate with the pinned published Merman
release. Evaluate a branching flowchart, a labelled dependency graph, subgraphs,
a sequence with notes and `alt`/`loop`, Unicode labels, malformed syntax, and
unsupported constructs. Check usable widths of 40, 60, 80 and 120 display cells,
plus smaller parent regions. Evaluate State separately before adding it.

Record correctness, missing semantics, warm/cold render times, dependency/build
impact, cancellation behavior and resource refusals. Pin finite input, model,
grid and output budgets; choose their values from these results. A worker
deadline with cooperative checkpoints is not a hard kill guarantee.

Go forward only if common diagrams preserve labels, edge direction and
relationships, ordinary cases are readable, overflow is explicit, and work is
bounded. If it fails, compare mermansi against the same examples and report the
remaining tradeoff before broad integration. Do not build a custom Mermaid
parser/layout engine as a fallback.

### 2. Deliver the TUI Markdown MVP

- Add a focused diagram module with pure source-to-result operations and typed
  outcomes. Extract fences through `pulldown-cmark`, preserving source and block
  identity; do not use a regex as the Markdown parser.
- Add bounded background preparation owned by App/workflow state. Layout stays
  outside rendering and the input loop. Reuse an engine where safe, limit the
  queue/concurrency, and cancel or retire obsolete work.
- Identify a request by the owning document/workflow and revision, source
  digest, available display-cell width, relevant theme/options and renderer
  version. Apply results only to a still-matching request. Cache failures as
  well as successes within a bounded memory budget.
- Extend Markdown rendering to consume ready artifacts and show a pending or
  source fallback on misses. Start with the standalone viewer and plan review
  gate. Its existing width/line cache needs explicit invalidation when diagram
  completion changes the document, not only when width changes.
- Render plain diagram text as Ratatui lines/styles; never print renderer ANSI
  directly to the TUI. Account for list/quote/frame prefixes in usable width.
  Avoid the current prose renderer's 24-column minimum becoming permission to
  draw a diagram wider than its actual region.
- Add source mode and actionable fallback messages while preserving raw
  Markdown storage, plan editing, approval behavior and navigation.

### 3. Extend readers and add desktop SVG

- Apply the same prepared-result policy to Learning and review surfaces.
  Several review renderers call `render_markdown` during drawing; merely adding
  layout inside that function would introduce repeated work on redraw.
- Add one lazily loaded desktop diagram component shared by both Markdown
  paths. Bundle a pinned official Mermaid release locally; validate its browser
  requirements against supported Tauri WebViews before selecting the version.
- Render visible diagrams on demand, cache by source/options/theme, use unique
  SVG IDs and discard stale asynchronous completions on source/view changes.
  Do not assume an async JavaScript API moves layout off the WebView thread;
  bound diagram size and concurrent work, and evaluate responsiveness.
- Keep source escaped. Configure strict Mermaid rendering, disable interactive
  source callbacks and remote assets, and validate generated SVG admission.
  Retain the desktop's current CSP and PR sanitization boundaries. Its CSP
  currently disallows frames, so Mermaid's iframe sandbox mode is not a
  drop-in choice. Test adversarial labels/configuration and duplicate diagrams.
- Keep rendering offline with bundled fonts/assets as needed; no diagram source
  is sent to Mermaid Live, Mermaid Chart, a public renderer or a CDN at runtime.

### 4. Make diagrams useful in agent workflows

After the readers work, allow optional small Mermaid diagrams in Learning
explanations and plan Architecture sections when they make the answer clearer.
Use the common terminal subset by default and always include a prose explanation.
Do not require every response to contain a chart.

Clarify plan prompts to say that the **whole response** must not be wrapped in a
code fence, while internal Mermaid fences are allowed. Current defaults and
legacy templates in `src/prompts/defaults.rs` and `src/plan_interview.rs` say
"no fenced code block." Verify synthesis, revision, critique, quick synthesis
JSON and wrapper normalization preserve internal fences. Respect existing prompt
overrides; do not silently rewrite user-authored templates.

Keep automatically generated feature/session graphs, runtime telemetry charts,
drag-and-drop editing, `.mmd` file discovery, export and terminal image protocols
as separately scoped extensions. They are not necessary to read Mermaid in AMF.

## Acceptance and validation

- Correctly identify fences, including tilde/long fences, CRLF, nested list and
  quote contexts, and multiple diagrams. Preserve non-Mermaid code behavior.
- Verify labels, endpoints, direction and branch semantics for the supported
  subset, not only the presence of box-drawing characters.
- Verify narrow/wide layouts, width accounting for Unicode, fallback/source
  mode, scroll bounds and resize behavior. Never silently wrap connector rows.
- Verify invalid syntax, unsupported semantics, oversized input, finite output,
  cancellation and stale results after close, refresh, theme or workflow changes.
- Verify ordinary key/pane responsiveness while diagram work runs, and that
  repeated redraws do not repeatedly lay out the same diagram.
- For desktop work, test shared component lifecycle and surrounding Markdown
  behavior with Vitest, then exercise actual SVG rendering in a WebView/browser;
  DOM-only mocks cannot prove font measurement or graphical layout. Check offline
  rendering, source access, enlargement, CSP and hostile diagram inputs.
- Run focused Rust Markdown, plans, Learning and review suites while iterating.
  At code milestones run `cargo test --workspace --locked`, `cargo fmt --check`
  and strict workspace/all-target Clippy as required by AGENTS.md. For frontend
  changes also run the GUI test suite and `npm run build`.

Initial success means a user can open a plan containing a supported Mermaid
flowchart or sequence, understand it without leaving AMF, inspect its source,
and keep reading if another block fails. Graphical terminal parity is not an
acceptance requirement for this proposal.

## Decisions for user review

- Recommended initial priority: TUI viewer and plan gate, then Learning/review
  and desktop. Desktop-first is a viable alternative if graphical fidelity is
  the main reason for this feature.
- Recommended initial scope: Flowchart and Sequence, with State only if it
  passes evaluation. Other families retain source fallback in the TUI.
- Recommended dependency path: evaluate Merman first; use official Mermaid for
  desktop SVG. A shared Rust-to-SVG desktop backend remains an alternative if
  avoiding two renderers outweighs upstream graphical compatibility.
- Recommended workflow behavior: consume existing Mermaid first, then adjust
  built-in generation prompts to permit optional diagrams.

Acceptance authorizes the evaluation and this staged integration. Terminal
images, automatic dashboards and a diagram editor require a separate plan.

## Sources

Upstream pages and registry metadata were checked on 2026-10-10. Capability
claims are upstream documentation, not an AMF-tested compatibility guarantee.

- [Mermaid usage, SVG API, security modes and runtime requirements](https://mermaid.ai/open-source/config/usage.html)
- [Merman project and embedding entry point](https://github.com/Latias94/merman)
- [Merman 0.8.0 terminal renderer contract](https://github.com/Latias94/merman/blob/v0.8.0/crates/merman-ascii/README.md)
- [Merman host integration examples and scheduling boundaries](https://github.com/Latias94/merman/blob/main/crates/merman/examples/README.md)
- [Merman published version metadata](https://crates.io/api/v1/crates/merman/0.8.0)
- [mermansi manifest and pinned alpha dependencies](https://github.com/kingsword09/mermansi/blob/main/Cargo.toml)
- [Native mermaid-rs-renderer](https://github.com/quangdang46/mermaid-rs-renderer)
- [Official Mermaid CLI](https://github.com/mermaid-js/mermaid-cli)
- [Ratatui image protocols and terminal compatibility](https://github.com/ratatui/ratatui-image)
- [Ratatui image development dependency manifest](https://github.com/ratatui/ratatui-image/blob/master/Cargo.toml)
