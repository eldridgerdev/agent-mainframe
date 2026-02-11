# Language & TUI Framework Comparison

## Languages

### TypeScript + Node.js

**Pros:**

- Fastest time-to-MVP; huge ecosystem of CLI/TUI libraries
- Type safety catches bugs at compile time
- Claude Code itself is Node.js - easy to integrate/extend
- npm ecosystem has tmux wrappers (`node-tmux`), git helpers,
  and dozens of TUI frameworks
- Hot reloading with `tsx` during development
- Async/await makes managing child processes natural
- Large community; easy to find contributors

**Cons:**

- Startup time ~50-100ms (noticeable for quick CLI commands)
- Runtime dependency (Node.js v18+ required)
- `node_modules` bloat
- Memory overhead ~15-30MB per invocation
- Single-threaded; CPU-bound work blocks the event loop
- Package churn; dependencies may break over time

---

### Rust

**Pros:**

- Single static binary, zero runtime dependencies
- Startup time <5ms; extremely fast execution
- Memory-safe without garbage collection
- Excellent TUI ecosystem: `ratatui` (actively maintained,
  community standard), `crossterm` (cross-platform terminal
  backend)
- Low memory footprint (~2-5MB)
- `tokio` for async process management
- Mature CLI parsing with `clap`
- Great error handling with `Result`/`Option` types
- Growing ecosystem for terminal apps (gitui, lazygit-like
  tools are Rust-based)

**Cons:**

- Steeper learning curve; slower initial development
- Longer compile times (2-5 min for full build)
- Borrow checker can be frustrating for rapid prototyping
- Smaller ecosystem for high-level abstractions
- Harder to script/extend at runtime
- No REPL-driven development

---

### Go

**Pros:**

- Single static binary, no runtime needed
- Fast startup (<10ms) and low memory
- Excellent TUI library: `bubbletea` (by Charm, very popular,
  Elm-architecture, composable)
- Simple language; easy to read and maintain
- Built-in concurrency with goroutines (great for managing
  multiple processes)
- Fast compilation (<30s typically)
- Strong CLI ecosystem: `cobra`, `viper` for config
- Cross-compilation is trivial

**Cons:**

- Verbose error handling (`if err != nil` everywhere)
- No generics until recently; some patterns are clunky
- Less expressive type system than Rust or TypeScript
- No sum types / pattern matching (enums are weak)
- Dependency management with Go modules can be surprising
- Testing framework is minimal compared to others

---

### Bash/Shell

**Pros:**

- Zero dependencies; works everywhere
- Trivial to prototype (50-100 lines for MVP)
- Direct tmux/git integration without wrappers
- No build step; instant iteration
- Smallest possible footprint
- Easy to understand and modify

**Cons:**

- No type safety; bugs hide until runtime
- Hard to maintain past ~500 lines
- Error handling is fragile (`set -e` has gotchas)
- No real data structures (arrays are painful)
- Testing is difficult
- Building a TUI dashboard is impractical
- String manipulation is error-prone
- Not suitable for the custom TUI dashboard requirement

---

## TUI Frameworks

### Ratatui (Rust)

- **Stars:** 12k+ | **Status:** Very active
- **Architecture:** Immediate-mode rendering; you draw each
  frame explicitly
- **Strengths:** Extremely performant, pixel-perfect layouts,
  extensive widget library (tables, charts, tabs, popups),
  great documentation, used by production tools
- **Weaknesses:** More boilerplate than reactive frameworks;
  you manage state yourself
- **Notable apps built with it:** gitui, wiki-tui, taskwarrior-tui
- **Best for:** High-performance dashboards, complex layouts

### Bubbletea (Go)

- **Stars:** 28k+ | **Status:** Very active (Charm.sh team)
- **Architecture:** Elm Architecture (Model-Update-View);
  reactive and composable
- **Strengths:** Beautiful component library (`bubbles`),
  `lipgloss` for styling, excellent documentation, very
  ergonomic API, large community
- **Weaknesses:** Go's type system limits some patterns;
  complex state management can get verbose
- **Notable apps built with it:** gum, soft-serve, wishlist
- **Best for:** Interactive CLIs, beautiful terminal UIs

### Ink (TypeScript/React)

- **Stars:** 27k+ | **Status:** Active
- **Architecture:** React component model adapted for terminal
- **Strengths:** Familiar React patterns, component reuse,
  hooks, flexbox layout, `ink-ui` component library
- **Weaknesses:** React overhead in terminal context, limited
  to what React model supports, no mouse support, rendering
  can flicker with complex layouts
- **Notable apps built with it:** Gatsby CLI, Prisma CLI,
  Shopify CLI
- **Best for:** Developers who know React, moderate-complexity
  TUIs

### Blessed / Neo-Blessed (Node.js)

- **Stars:** 11k+ | **Status:** Maintained but slow
- **Architecture:** Widget-based (like traditional GUI toolkits)
- **Strengths:** Rich widget set (forms, tables, file managers),
  good mouse support, works well for dashboards
- **Weaknesses:** Older API design, documentation gaps, fewer
  recent updates, memory hungry for complex UIs
- **Best for:** Dashboard-style UIs, monitoring tools

### Textual (Python)

- **Stars:** 25k+ | **Status:** Very active (Textualize team)
- **Architecture:** CSS-styled widgets, async-first
- **Strengths:** Beautiful out of the box, CSS for styling,
  web-like dev experience, excellent docs
- **Weaknesses:** Python runtime required, slower startup,
  not ideal for system tools
- **Best for:** Python developers, data-heavy dashboards

---

## Recommendation Matrix

| Criteria              | TypeScript | Rust     | Go       | Bash |
|-----------------------|------------|----------|----------|------|
| Time to MVP           | Fast       | Slow     | Medium   | Fast |
| TUI quality           | Good (Ink) | Best     | Great    | N/A  |
| Performance           | OK         | Best     | Great    | Good |
| Maintainability       | Good       | Good     | Good     | Poor |
| Single binary         | No         | Yes      | Yes      | Yes  |
| Learning curve        | Low        | High     | Low      | Low  |
| Process management    | Good       | Good     | Best     | OK   |
| Extensibility         | Best       | Good     | Good     | Poor |
| Claude Code compat    | Best       | OK       | OK       | Good |

### Summary

- **Rust + Ratatui** if you want maximum performance and a
  polished, professional TUI. Higher upfront investment but
  results in the best end product.
- **Go + Bubbletea** if you want a great TUI with faster
  development time. The Elm architecture is elegant and the
  Charm ecosystem is excellent.
- **TypeScript + Ink** if you want the fastest MVP and are
  comfortable with React patterns. Easiest to integrate with
  Claude Code's own ecosystem.
- **Bash** is ruled out for the TUI dashboard requirement
  but could serve as a quick prototype for the core logic.
