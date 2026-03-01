# AMF Themes for OpenCode

This directory contains custom AMF themes for opencode with transparent backgrounds,
designed to work well when running opencode inside AMF (Agent Mainframe).

## Available Themes

- **amf** - Nord-based theme with transparent background
- **amf-tokyonight** - Tokyo Night with transparent background  
- **amf-catppuccin** - Catppuccin Mocha with transparent background

## How to Use

Set the theme in `.opencode/tui.json`:

```json
{
  "$schema": "https://opencode.ai/tui.json",
  "theme": "amf-tokyonight"
}
```

Or use the `/theme` command in opencode to select a theme interactively.

## Creating Custom AMF Themes

To create your own AMF variant of a built-in theme:

1. Find the built-in theme you want to modify
2. Copy its color definitions
3. Set these background properties to `"none"`:
   - `background`
   - `backgroundPanel` (optional - can use a subtle color for contrast)
   - `backgroundElement`
   - `diffAddedBg`
   - `diffRemovedBg`
   - `diffContextBg`
   - `diffAddedLineNumberBg`
   - `diffRemovedLineNumberBg`
   - `markdownCodeBlock`

## Theme Priority

Themes are loaded in this order (later overrides earlier):

1. Built-in themes (embedded in opencode binary)
2. `~/.config/opencode/themes/*.json`
3. `<project>/.opencode/themes/*.json`
4. `./.opencode/themes/*.json`

## Testing

To test if a theme works correctly in AMF:

1. Start AMF
2. Create/select a feature
3. Press `Enter` to view the feature (switches to opencode)
4. Check that the background is transparent and text is readable
5. Try different panels and views to ensure good contrast
