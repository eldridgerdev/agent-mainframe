// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import TerminalPane from "../src/TerminalPane";

const term = vi.hoisted(() => ({
  cols: 80, rows: 24,
  loadAddon: vi.fn(), open: vi.fn(), reset: vi.fn(),
  write: vi.fn((_data: string, callback?: () => void) => callback?.()), refresh: vi.fn(),
  onData: vi.fn(() => ({ dispose: vi.fn() })),
  onResize: vi.fn(() => ({ dispose: vi.fn() })), dispose: vi.fn(),
  onScroll: vi.fn(() => ({ dispose: vi.fn() })), attachCustomKeyEventHandler: vi.fn(),
  scrollToBottom: vi.fn(), buffer: { active: { viewportY: 0, baseY: 0 } },
}));
vi.mock("@xterm/xterm", () => ({ Terminal: class { constructor() { return term; } } }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit = vi.fn(); } }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
});
afterEach(() => { cleanup(); vi.clearAllMocks(); vi.unstubAllGlobals(); });

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
