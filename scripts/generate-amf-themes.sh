#!/bin/bash
set -e

THEMES_DIR=".opencode/themes"
BUILT_IN_THEMES=(
  "tokyonight"
  "everforest"
  "ayu"
  "catppuccin"
  "catppuccin-macchiato"
  "gruvbox"
  "kanagawa"
  "nord"
  "matrix"
  "one-dark"
)

mkdir -p "$THEMES_DIR"

echo "Generating AMF theme variants..."

for theme in "${BUILT_IN_THEMES[@]}"; do
  output_file="$THEMES_DIR/amf-${theme}.json"
  
  echo "Creating amf-${theme} theme..."
  
  cat > "$output_file" << EOF
{
  "\$schema": "https://opencode.ai/theme.json",
  "extends": "${theme}",
  "theme": {
    "background": "none",
    "backgroundPanel": "none",
    "backgroundElement": "none",
    "diffAddedBg": "none",
    "diffRemovedBg": "none",
    "diffContextBg": "none",
    "diffAddedLineNumberBg": "none",
    "diffRemovedLineNumberBg": "none",
    "markdownCodeBlock": "none"
  }
}
EOF
  
  echo "  Created $output_file"
done

echo ""
echo "All AMF themes generated successfully!"
echo "To use a theme, set 'theme' in .opencode/tui.json to one of:"
for theme in "${BUILT_IN_THEMES[@]}"; do
  echo "  - amf-${theme}"
done
