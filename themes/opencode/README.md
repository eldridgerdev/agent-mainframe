# AMF OpenCode Themes

This directory contains AMF-specific themes for opencode that are embedded in the AMF binary.

These themes are designed specifically for use with opencode when running inside AMF's embedded tmux view. They feature transparent backgrounds that work well with AMF's terminal interface.

## How It Works

When AMF starts or creates a feature, it automatically injects these themes into the worktree's `.opencode/themes/` directory. This ensures that anyone using AMF has access to transparent-background themes that work well in the AMF environment.

## Available Themes

- **amf.json** - Nord-based theme with transparent background
- **amf-tokyonight.json** - Tokyo Night with transparent background  
- **amf-catppuccin.json** - Catppuccin Mocha with transparent background

## Theme Properties

All themes have the **main background** set to `"none"` for transparency, while all other UI elements use proper theme colors:

### Transparent Background
- `background` - Main background (transparent)

### Theme-Colored Backgrounds
- `backgroundPanel` - Panel backgrounds (uses theme colors)
- `backgroundElement` - UI element backgrounds (uses theme colors)
- `diffAddedBg` - Diff addition backgrounds (uses theme colors)
- `diffRemovedBg` - Diff removal backgrounds (uses theme colors)
- `diffContextBg` - Diff context backgrounds (uses theme colors)
- `diffAddedLineNumberBg` - Diff line number backgrounds (uses theme colors)
- `diffRemovedLineNumberBg` - Diff line number backgrounds (uses theme colors)
- `markdownCodeBlock` - Markdown code block backgrounds (uses theme colors)

This approach provides:
- **Visual hierarchy**: Panels and elements are visually distinct
- **Better readability**: Diff sections and code blocks have proper backgrounds
- **Theme consistency**: All colors match their respective theme palettes
- **Transparency**: Main background shows AMF terminal through

## Usage in OpenCode

Once AMF injects the themes, users can select them in opencode using:

1. The `/theme` command in opencode
2. Editing `.opencode/tui.json` in the worktree:

```json
{
  "theme": "amf-catppuccin"
}
```

## Directory Structure

```
themes/
└── opencode/           # Themes for opencode agent
    ├── amf.json
    ├── amf-tokyonight.json
    ├── amf-catppuccin.json
    └── README.md       # This file
```

## Updating Themes

To update the embedded themes:

1. Edit the JSON files in `themes/opencode/`
2. Rebuild AMF: `cargo build --release`
3. The new themes will be embedded in the binary

## Adding New Themes

To add a new opencode theme:

1. Create the theme JSON file in `themes/opencode/`
2. Add it to the `theme_files` array in `src/theme.rs`:
   ```rust
   ("amf-newtheme.json", include_str!("../themes/opencode/amf-newtheme.json")),
   ```
3. Rebuild AMF

## Implementation

Themes are injected in `src/app/mod.rs` in the `ensure_feature_running` function using the `ThemeManager` from `src/theme.rs`.

The injection happens before tmux sessions are created, ensuring themes are available as soon as the opencode agent starts.
