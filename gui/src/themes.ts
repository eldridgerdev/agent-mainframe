import { useSyncExternalStore } from "react";
import type { ITheme } from "@xterm/xterm";
import { DARK_TERMINAL_THEME, LIGHT_TERMINAL_THEME } from "./terminalTheme";

export interface GuiTheme {
  id: string;
  name: string;
  mode: "light" | "dark" | "system";
  tokens: Record<string, string>;
  terminal: ITheme;
}
export interface ThemeCatalog {
  tui_theme: string;
  themes: GuiTheme[];
  errors: string[];
  directory: string;
}
const key = "amf.gui.theme";
const listeners = new Set<() => void>();
let preference: string | undefined;
export function readThemePreference(): string {
  if (preference !== undefined) return preference;
  try { preference = window.localStorage.getItem(key) || "follow-tui"; }
  catch { preference = "follow-tui"; }
  return preference;
}
export function setThemePreference(value: string) {
  preference = value;
  try {
    if (value === "follow-tui") window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch { /* In-memory switching still works when storage is unavailable. */ }
  listeners.forEach((listener) => listener());
}
export function useThemePreference() {
  return useSyncExternalStore((listener) => {
    listeners.add(listener);
    return () => { listeners.delete(listener); };
  }, readThemePreference, () => "follow-tui");
}
export function resolveTheme(catalog: ThemeCatalog | undefined, choice: string): GuiTheme | undefined {
  if (["light", "dark", "system"].includes(choice)) {
    return { id: choice, name: choice, mode: choice as GuiTheme["mode"], tokens: {}, terminal: {} };
  }
  return catalog?.themes.find((theme) => theme.id === (choice === "follow-tui" ? catalog.tui_theme : choice));
}
let active: GuiTheme | undefined;
let appliedTokens: string[] = [];
export function terminalAppearance(systemDark = window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false) {
  const dark = active?.mode === "dark" || ((!active || active.mode === "system") && systemDark);
  return {
    theme: { ...(dark ? DARK_TERMINAL_THEME : LIGHT_TERMINAL_THEME), ...active?.terminal },
    minimumContrastRatio: dark ? 4.5 : 1,
  };
}
export function applyTheme(theme: GuiTheme | undefined) {
  active = theme;
  const root = document.documentElement;
  for (const token of appliedTokens) root.style.removeProperty(`--${token}`);
  appliedTokens = Object.keys(theme?.tokens ?? {});
  if (!theme || theme.mode === "system") delete root.dataset.theme;
  else root.dataset.theme = theme.mode;
  for (const [token, color] of Object.entries(theme?.tokens ?? {})) root.style.setProperty(`--${token}`, color);
  // The terminal surround must follow a custom terminal background too.
  root.style.removeProperty("--terminal-bg");
  if (theme?.terminal.background) root.style.setProperty("--terminal-bg", theme.terminal.background);
  else if (theme?.tokens["terminal-bg"]) root.style.setProperty("--terminal-bg", theme.tokens["terminal-bg"]);
  window.dispatchEvent(new Event("amf-theme-change"));
}
export function resetThemesForTest() {
  preference = undefined;
  applyTheme(undefined);
}
