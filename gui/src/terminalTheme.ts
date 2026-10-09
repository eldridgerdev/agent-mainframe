import type { ITheme } from "@xterm/xterm";

// Preserve the existing terminal palette in light mode.
export const LIGHT_TERMINAL_THEME: ITheme = {
  background: "#0d1014",
  foreground: "#d7dce2",
  cursor: "#9aa7ff",
  selectionBackground: "#2c3550",
  black: "#1b1f25",
  red: "#f47067",
  green: "#57c27a",
  yellow: "#d9b34c",
  blue: "#6cb6ff",
  magenta: "#c79bf5",
  cyan: "#56c5d0",
  white: "#c9d1d9",
  brightBlack: "#636e7b",
  brightRed: "#ff938a",
  brightGreen: "#78d796",
  brightYellow: "#e8c66b",
  brightBlue: "#8ecbff",
  brightMagenta: "#dcbdfb",
  brightCyan: "#7fdbe3",
  brightWhite: "#f0f3f6",
};

// The dark terminal shares the page background. ANSI black remains readable
// when applications use it as foreground; bright black is used for dim output.
export const DARK_TERMINAL_THEME: ITheme = {
  background: "#242932", foreground: "#f1f4f8",
  cursor: "#c5ccff", cursorAccent: "#242932",
  selectionBackground: "#566584", selectionForeground: "#ffffff",
  black: "#aab6c6", red: "#ffaaa2", green: "#88e0a8",
  yellow: "#ffd17d", blue: "#a6caff", magenta: "#d4b6ff",
  cyan: "#90dce5", white: "#dce3ed", brightBlack: "#c1cad6",
  brightRed: "#ffc0b9", brightGreen: "#a3efbd", brightYellow: "#ffe0a3",
  brightBlue: "#c1dcff", brightMagenta: "#e5d0ff", brightCyan: "#b5eef4",
  brightWhite: "#ffffff",
};
