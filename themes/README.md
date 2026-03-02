# AMF Embedded Themes

This directory contains theme files that are embedded in the AMF binary and automatically injected into worktrees.

## Structure

```
themes/
├── opencode/           # Themes for opencode agent
│   ├── amf.json
│   ├── amf-tokyonight.json
│   ├── amf-catppuccin.json
│   └── README.md
└── README.md          # This file
```

## Purpose

AMF embeds these themes at compile time and injects them into every worktree when a feature is started. This ensures:

- **Consistency**: All AMF users have access to the same themes
- **Convenience**: No manual theme installation required
- **Transparency**: Themes are optimized for AMF's embedded terminal view

## Supported Agents

### OpenCode (`themes/opencode/`)

Themes for the opencode AI coding agent. These themes feature transparent backgrounds that work well when viewing opencode inside AMF's tmux integration.

See `themes/opencode/README.md` for details.

## Adding Support for Other Agents

To add themes for other agents:

1. Create a subdirectory: `themes/<agent-name>/`
2. Add theme JSON files
3. Update `src/theme.rs` to inject the new themes
4. Add a README explaining the themes

## Build Integration

Themes are embedded using Rust's `include_str!()` macro:

```rust
let theme_files = [
    ("amf.json", include_str!("../themes/opencode/amf.json")),
    // ...
];
```

This means:
- Themes are compiled into the binary
- No runtime file dependencies
- Binary size impact is minimal (~13KB for current themes)
