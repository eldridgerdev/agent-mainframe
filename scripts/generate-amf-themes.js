const fs = require('fs');
const path = require('path');
const https = require('https');

const BUILT_IN_THEMES = [
  'tokyonight',
  'everforest', 
  'ayu',
  'catppuccin',
  'catppuccin-macchiato',
  'gruvbox',
  'kanagawa',
  'nord',
  'matrix',
  'one-dark'
];

const THEMES_DIR = path.join(__dirname, '..', '.opencode', 'themes');

function fetchTheme(themeName) {
  return new Promise((resolve, reject) => {
    const url = `https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/tui/src/theme/builtin/${themeName}.ts`;
    
    https.get(url, (res) => {
      let data = '';
      res.on('data', chunk => data += chunk);
      res.on('end', () => {
        if (res.statusCode === 200) {
          resolve(data);
        } else {
          reject(new Error(`Failed to fetch ${themeName}: ${res.statusCode}`));
        }
      });
    }).on('error', reject);
  });
}

function createAMFTheme(baseTheme) {
  return {
    "$schema": "https://opencode.ai/theme.json",
    ...baseTheme,
    theme: {
      ...baseTheme.theme,
      background: "none",
      backgroundPanel: "none",
      backgroundElement: "none",
      diffAddedBg: "none",
      diffRemovedBg: "none",
      diffContextBg: "none",
      diffAddedLineNumberBg: "none",
      diffRemovedLineNumberBg: "none",
      markdownCodeBlock: "none"
    }
  };
}

async function main() {
  console.log('Generating AMF theme variants...\n');
  
  if (!fs.existsSync(THEMES_DIR)) {
    fs.mkdirSync(THEMES_DIR, { recursive: true });
  }
  
  for (const themeName of BUILT_IN_THEMES) {
    try {
      console.log(`Creating amf-${themeName} theme...`);
      
      // For now, create a simple wrapper theme
      // In the future, we could fetch the actual theme definition
      const amfTheme = {
        "$schema": "https://opencode.ai/theme.json",
        extends: themeName,
        theme: {
          background: "none",
          backgroundPanel: "none",
          backgroundElement: "none",
          diffAddedBg: "none",
          diffRemovedBg: "none",
          diffContextBg: "none",
          diffAddedLineNumberBg: "none",
          diffRemovedLineNumberBg: "none",
          markdownCodeBlock: "none"
        }
      };
      
      const outputPath = path.join(THEMES_DIR, `amf-${themeName}.json`);
      fs.writeFileSync(outputPath, JSON.stringify(amfTheme, null, 2));
      console.log(`  Created ${outputPath}`);
    } catch (error) {
      console.error(`  Error: ${error.message}`);
    }
  }
  
  console.log('\nAll AMF themes generated!');
  console.log('\nTo use a theme, set "theme" in .opencode/tui.json to one of:');
  BUILT_IN_THEMES.forEach(theme => {
    console.log(`  - amf-${theme}`);
  });
}

main().catch(console.error);
