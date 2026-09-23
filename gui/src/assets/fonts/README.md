# Terminal fallback fonts

Bundled so agent TUIs render the same on every machine, whatever monospace
font is installed. They are fallbacks only: the terminal's text font comes
from the system (see `TERMINAL_FONT` in `../../TerminalPane.tsx`).

| File | Source | License |
| --- | --- | --- |
| `SymbolsNerdFontMono-Regular.woff2` | Nerd Fonts "Symbols Only" (Symbols Nerd Font Mono), converted to WOFF2 | MIT, `SymbolsNerdFont-LICENSE.txt` |
| `NotoSansSymbols2-subset.woff2` | Noto Sans Symbols 2 | SIL OFL 1.1, `NotoSansSymbols-OFL.txt` |
| `NotoSansSymbols-subset.woff2` | Noto Sans Symbols | SIL OFL 1.1, `NotoSansSymbols-OFL.txt` |

The Noto files are subset to arrows, technical symbols, enclosed
alphanumerics, geometric shapes, dingbats and miscellaneous symbols
(`U+2190-21FF,U+2300-23FF,U+2460-24FF,U+25A0-25FF,U+2600-27BF,U+2900-297F,U+2B00-2BFF`)
with `pyftsubset --flavor=woff2` from fontTools.
