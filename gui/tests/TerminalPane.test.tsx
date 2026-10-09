// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { applyTheme, resetThemesForTest } from "../src/themes";
import TerminalPane from "../src/TerminalPane";

const term = vi.hoisted(() => ({
  cols: 80, rows: 24, options: { theme: {}, minimumContrastRatio: 1 },
  loadAddon: vi.fn(), open: vi.fn(), reset: vi.fn(),
  write: vi.fn((_data: string, callback?: () => void) => callback?.()), refresh: vi.fn(),
  onData: vi.fn(() => ({ dispose: vi.fn() })),
  onResize: vi.fn(() => ({ dispose: vi.fn() })), dispose: vi.fn(),
  onScroll: vi.fn(() => ({ dispose: vi.fn() })), attachCustomKeyEventHandler: vi.fn(),
  scrollToBottom: vi.fn(), buffer: { active: { viewportY: 0, baseY: 0 } },
}));
vi.mock("@xterm/xterm", () => ({ Terminal: class { constructor(options: { theme: object; minimumContrastRatio: number }) { term.options = options; return term; } } }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit = vi.fn(); } }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

beforeEach(() => {
  resetThemesForTest();
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
});
afterEach(() => { cleanup(); resetThemesForTest(); vi.clearAllMocks(); vi.unstubAllGlobals(); });

const target = { project_id: "project", feature_id: "feature", session_id: "agent" };
const frame = (replay: string) => ({ replay, alternate_screen: false, mouse_reporting: false });

it("enables the composer after attachment and disables it on detach", async () => {
  let finishAttach!: (response: unknown) => void;
  vi.mocked(invoke).mockImplementation((command) => command === "attach_terminal"
    ? new Promise((resolve) => { finishAttach = resolve; }) : Promise.resolve());
  const ready = vi.fn();
  const view = render(<TerminalPane target={target} onReadyChange={ready} />);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("attach_terminal", { target, size: { cols: 80, rows: 24 } }));
  expect(ready.mock.calls).toEqual([[false]]);
  await act(async () => finishAttach({ key: "feature:agent", generation: 42, initial: frame("Agent ready") }));
  expect(term.write).toHaveBeenCalledWith("Agent ready", expect.any(Function));
  expect(ready.mock.calls).toEqual([[false], [true]]);
  view.unmount();
  expect(ready.mock.calls).toEqual([[false], [true], [false]]);
  expect(invoke).toHaveBeenCalledWith("detach_terminal", { key: "feature:agent", generation: 42 });
});

it("leaves sending disabled when attachment fails", async () => {
  vi.mocked(invoke).mockRejectedValue({ kind: "not_found", message: "Session stopped" });
  const ready = vi.fn();
  render(<TerminalPane target={target} onReadyChange={ready} />);
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Terminal connection failed: Session stopped");
  expect(ready.mock.calls).toEqual([[false]]);
});

it("never enables a composer whose attachment finishes after leaving the tab", async () => {
  let finishAttach!: (response: unknown) => void;
  vi.mocked(invoke).mockImplementation((command) => command === "attach_terminal"
    ? new Promise((resolve) => { finishAttach = resolve; }) : Promise.resolve());
  const ready = vi.fn();
  const view = render(<TerminalPane target={target} onReadyChange={ready} />);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("attach_terminal", { target, size: { cols: 80, rows: 24 } }));
  view.unmount();
  await act(async () => finishAttach({ key: "feature:agent", generation: 42, initial: frame("Agent ready") }));
  expect(ready.mock.calls.every(([value]) => value === false)).toBe(true);
  expect(invoke).toHaveBeenCalledWith("detach_terminal", { key: "feature:agent", generation: 42 });
  expect(term.write).not.toHaveBeenCalled();
});

it("follows system dark mode in place without reattaching or resetting history", async () => {
  const { DARK_TERMINAL_THEME, LIGHT_TERMINAL_THEME } = await import("../src/terminalTheme");
  let changed!: () => void;
  const media = { matches: true, addEventListener: vi.fn((_event, listener) => { changed = listener; }), removeEventListener: vi.fn() };
  vi.stubGlobal("matchMedia", vi.fn(() => media));
  vi.mocked(invoke).mockImplementation((command) => Promise.resolve(command === "attach_terminal"
    ? { key: "feature:agent", generation: 1, initial: frame("Existing history") } : undefined));
  const view = render(<TerminalPane target={target} />);
  await waitFor(() => expect(term.write).toHaveBeenCalled());
  expect(term.options.theme).toEqual(DARK_TERMINAL_THEME);
  expect(term.options.minimumContrastRatio).toBe(4.5);
  const resets = term.reset.mock.calls.length;
  const writes = term.write.mock.calls.length;
  act(() => { media.matches = false; changed(); });
  expect(term.options.theme).toEqual(LIGHT_TERMINAL_THEME);
  expect(term.options.minimumContrastRatio).toBe(1);
  act(() => { media.matches = true; changed(); });
  expect(term.options.theme).toEqual(DARK_TERMINAL_THEME);
  expect(term.options.minimumContrastRatio).toBe(4.5);
  expect(term.reset).toHaveBeenCalledTimes(resets);
  expect(term.write).toHaveBeenCalledTimes(writes);
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "attach_terminal")).toHaveLength(1);
  view.unmount();
  expect(media.removeEventListener).toHaveBeenCalledWith("change", changed);
});

it("switches a named palette live without attaching again or touching terminal contents", async () => {
  vi.mocked(invoke).mockImplementation((command) => Promise.resolve(command === "attach_terminal"
    ? { key: "feature:agent", generation: 1, initial: frame("Existing history") } : undefined));
  render(<TerminalPane target={target} />);
  await waitFor(() => expect(term.write).toHaveBeenCalled());
  const resets = term.reset.mock.calls.length;
  const writes = term.write.mock.calls.length;
  act(() => applyTheme({ id: "nord", name: "Nord", mode: "dark", tokens: {}, terminal: { background: "#2e3440", cyan: "#88c0d0" } }));
  expect(term.options.theme).toMatchObject({ background: "#2e3440", cyan: "#88c0d0" });
  expect(term.options.minimumContrastRatio).toBe(4.5);
  expect(term.reset).toHaveBeenCalledTimes(resets);
  expect(term.write).toHaveBeenCalledTimes(writes);
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "attach_terminal")).toHaveLength(1);
});
