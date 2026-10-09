// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import ThemePicker, { useGuiTheme } from "../src/ThemePicker";
import { applyTheme, readThemePreference, resetThemesForTest, resolveTheme, setThemePreference, terminalAppearance, type ThemeCatalog } from "../src/themes";
import { DARK_TERMINAL_THEME, LIGHT_TERMINAL_THEME } from "../src/terminalTheme";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const catalog: ThemeCatalog = {
  tui_theme: "nord", directory: "/isolated/amf/gui-themes", errors: [],
  themes: [
    { id: "nord", name: "Nord", mode: "dark", tokens: { accent: "#88c0d0" }, terminal: { background: "#2e3440" } },
    { id: "latte", name: "Latte", mode: "light", tokens: { bg: "#eff1f5" }, terminal: {} },
    { id: "custom:ocean", name: "Ocean", mode: "dark", tokens: { accent: "#abcdef" }, terminal: { cyan: "#abcdef" } },
  ],
};
let client: QueryClient;
function Harness() { return <ThemePicker appearance={useGuiTheme()} onClose={() => {}} />; }
function mount() {
  client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return render(<QueryClientProvider client={client}><Harness /></QueryClientProvider>);
}
beforeEach(() => {
  window.localStorage.clear(); resetThemesForTest();
  vi.mocked(invoke).mockResolvedValue(catalog);
});
afterEach(() => { cleanup(); client?.clear(); resetThemesForTest(); vi.restoreAllMocks(); });
it("follows live TUI changes, then retains an independent GUI override until cleared", async () => {
  mount();
  await screen.findByRole("option", { name: "Nord" });
  await waitFor(() => expect(document.documentElement.style.getPropertyValue("--accent")).toBe("#88c0d0"));
  act(() => client.setQueryData(["theme-catalog"], { ...catalog, tui_theme: "latte" }));
  await waitFor(() => expect(document.documentElement.dataset.theme).toBe("light"));
  fireEvent.change(screen.getByLabelText("Theme"), { target: { value: "nord" } });
  expect(document.documentElement.dataset.theme).toBe("dark");
  act(() => client.setQueryData(["theme-catalog"], { ...catalog, tui_theme: "latte" }));
  expect(document.documentElement.dataset.theme).toBe("dark");
  expect(window.localStorage.getItem("amf.gui.theme")).toBe("nord");
  fireEvent.change(screen.getByLabelText("Theme"), { target: { value: "follow-tui" } });
  expect(document.documentElement.dataset.theme).toBe("light");
  expect(window.localStorage.getItem("amf.gui.theme")).toBeNull();
  expect(vi.mocked(invoke).mock.calls.every(([command]) => command === "theme_catalog")).toBe(true);
});
it("restores a saved choice across initialization and tolerates unavailable storage", () => {
  setThemePreference("custom:ocean"); resetThemesForTest();
  expect(readThemePreference()).toBe("custom:ocean");
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("disabled"); });
  setThemePreference("light");
  expect(readThemePreference()).toBe("light");
});
it("uses explicit light/dark independently of the system and follows system changes", () => {
  applyTheme(resolveTheme(catalog, "dark"));
  expect(terminalAppearance(false).theme).toEqual(DARK_TERMINAL_THEME);
  applyTheme(resolveTheme(catalog, "light"));
  expect(terminalAppearance(true).theme).toEqual(LIGHT_TERMINAL_THEME);
  applyTheme(resolveTheme(catalog, "system"));
  expect(document.documentElement.dataset.theme).toBeUndefined();
  expect(terminalAppearance(true).theme).toEqual(DARK_TERMINAL_THEME);
  expect(terminalAppearance(false).theme).toEqual(LIGHT_TERMINAL_THEME);
});
it("partial custom themes fall back per property and clear overrides when switched", () => {
  applyTheme(resolveTheme(catalog, "custom:ocean"));
  expect(terminalAppearance(false).theme).toEqual({ ...DARK_TERMINAL_THEME, cyan: "#abcdef" });
  expect(document.documentElement.style.getPropertyValue("--accent")).toBe("#abcdef");
  applyTheme(resolveTheme(catalog, "latte"));
  expect(document.documentElement.style.getPropertyValue("--accent")).toBe("");
  expect(document.documentElement.style.getPropertyValue("--bg")).toBe("#eff1f5");
});
it("reports broken and missing themes while retaining a usable picker", async () => {
  setThemePreference("custom:missing");
  vi.mocked(invoke).mockResolvedValue({ ...catalog, errors: ["bad.json: unknown token: bogus"] });
  mount();
  await screen.findByText("bad.json: unknown token: bogus");
  expect(screen.getByRole("status").textContent).toContain("unavailable");
  expect(document.documentElement.dataset.theme).toBeUndefined();
  fireEvent.change(screen.getByLabelText("Theme"), { target: { value: "dark" } });
  expect(document.documentElement.dataset.theme).toBe("dark");
});
it("recovers from catalog failure without blocking built-in choices", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("unavailable"));
  mount();
  await screen.findByRole("alert");
  fireEvent.change(screen.getByLabelText("Theme"), { target: { value: "light" } });
  expect(document.documentElement.dataset.theme).toBe("light");
  vi.mocked(invoke).mockResolvedValue(catalog);
  fireEvent.click(screen.getByText("Retry"));
  await screen.findByRole("option", { name: "Nord" });
});
it("reflects preferences from another GUI window", async () => {
  mount(); await screen.findByRole("option", { name: "Nord" });
  const setItem = vi.spyOn(Storage.prototype, "setItem");
  const removeItem = vi.spyOn(Storage.prototype, "removeItem");
  act(() => window.dispatchEvent(new StorageEvent("storage", { key: "amf.gui.theme", newValue: "light" })));
  expect(document.documentElement.dataset.theme).toBe("light");
  act(() => window.dispatchEvent(new StorageEvent("storage", { key: "amf.gui.theme", newValue: null })));
  expect(readThemePreference()).toBe("follow-tui");
  // Writing an external value back would bounce stale choices between windows.
  expect(setItem).not.toHaveBeenCalled();
  expect(removeItem).not.toHaveBeenCalled();
});

it("applies explicit dark chrome, syntax and sidebar tokens and restores light defaults", () => {
  const style = document.createElement("style");
  style.textContent = [
    readFileSync(resolve("src/styles.css"), "utf8"),
    readFileSync(resolve("src/syntax.css"), "utf8"),
    readFileSync(resolve("src/sessionSidebar.css"), "utf8"),
  ].join("\n");
  document.head.appendChild(style);
  try {
    applyTheme(resolveTheme(catalog, "dark"));
    const dark = getComputedStyle(document.documentElement);
    expect(dark.getPropertyValue("--bg").trim()).toBe("#242932");
    expect(dark.getPropertyValue("--syn-comment").trim()).toBe("#b9c5df");
    expect(dark.getPropertyValue("--sb-harness-claude").trim()).toBe("#ffb69b");
    applyTheme(resolveTheme(catalog, "light"));
    const light = getComputedStyle(document.documentElement);
    expect(light.getPropertyValue("--bg").trim()).toBe("#f6f7f9");
    expect(light.getPropertyValue("--syn-comment").trim()).toBe("#5a6574");
    expect(light.getPropertyValue("--sb-harness-claude").trim()).toBe("#c4643f");
  } finally { style.remove(); }
});
