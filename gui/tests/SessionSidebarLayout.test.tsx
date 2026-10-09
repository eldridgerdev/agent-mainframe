// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import App from "../src/App";
import SessionSidebar from "../src/SessionSidebar";
import TerminalPane from "../src/TerminalPane";
import { resetSidebarPrefsForTest } from "../src/sidebarPrefs";
import type { WorkspaceSnapshot } from "../src/api";
import type { SessionSidebarView } from "../src/sessionSidebarApi";

// A stand-in for xterm.js with just what refitting and scroll-back touch:
// FitAddon.fit() adopts the width the layout now gives the terminal and
// reports a resize, as the real addon does.
const fake = vi.hoisted(() => {
  const layout = { cols: 100 };
  class FakeTerminal {
    static current: FakeTerminal;
    cols = 100;
    rows = 24;
    element: HTMLElement | undefined;
    buffer = { active: { viewportY: 0, baseY: 0 } };
    lines = 0;
    resizeHandlers: ((size: { cols: number; rows: number }) => void)[] = [];
    scrollHandlers: (() => void)[] = [];
    constructor() { FakeTerminal.current = this; }
    loadAddon() {}
    refresh() {}
    dispose() {}
    open(parent: HTMLElement) {
      this.element = document.createElement("div");
      parent.appendChild(this.element);
    }
    reset() { this.lines = 0; this.buffer.active = { viewportY: 0, baseY: 0 }; }
    write(data: string, callback?: () => void) {
      this.lines += data.split("\r\n").length;
      this.buffer.active.baseY = Math.max(0, this.lines - this.rows);
      this.buffer.active.viewportY = this.buffer.active.baseY;
      callback?.();
    }
    scrollLines(amount: number) {
      const { baseY } = this.buffer.active;
      this.buffer.active.viewportY = Math.min(baseY, Math.max(0, this.buffer.active.viewportY + amount));
      this.scrollHandlers.forEach((handler) => handler());
    }
    scrollToBottom() { this.scrollLines(this.buffer.active.baseY); }
    onData() { return { dispose: () => {} }; }
    onResize(handler: (size: { cols: number; rows: number }) => void) {
      this.resizeHandlers.push(handler);
      return { dispose: () => {} };
    }
    onScroll(handler: () => void) {
      this.scrollHandlers.push(handler);
      return { dispose: () => {} };
    }
    attachCustomKeyEventHandler() {}
    fitTo(cols: number) {
      if (cols === this.cols) return;
      this.cols = cols;
      this.resizeHandlers.forEach((handler) => handler({ cols, rows: this.rows }));
    }
  }
  return { FakeTerminal, layout };
});
vi.mock("@xterm/xterm", () => ({ Terminal: fake.FakeTerminal }));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class { fit() { fake.FakeTerminal.current?.fitTo(fake.layout.cols); } },
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const PROMPT = "Run the sidebar screenshots";

function sidebarView(): SessionSidebarView {
  return {
    session_id: "claude", harness: "claude", title: "Claude Sidebar", notes: [],
    sections: [
      { kind: "status", title: "Status", actions: [], lines: [
        { kind: "field", label: "Activity", value: "Ready", tone: "ready", emphasised: true },
      ] },
      { kind: "prompt", title: "Prompt", actions: [{ kind: "reuse_prompt", prompt: PROMPT }], lines: [
        { kind: "text", text: PROMPT, tone: "plain", emphasised: false },
      ] },
    ],
  };
}

let observers: (() => void)[] = [];
let emit: (payload: unknown) => void = () => {};

beforeEach(() => {
  window.localStorage.clear();
  resetSidebarPrefsForTest();
  observers = [];
  fake.layout.cols = 100;
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: () => void) { observers.push(callback); }
    observe() {}
    disconnect() {}
  });
  vi.mocked(listen).mockImplementation(async (_event, handler) => {
    emit = (payload) => (handler as (event: { payload: unknown }) => void)({ payload });
    return () => {};
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

const calls = (name: string) => vi.mocked(invoke).mock.calls.filter(([command]) => command === name);
const relayout = (cols: number) => act(() => {
  fake.layout.cols = cols;
  observers.forEach((callback) => callback());
});

describe("agent tab layout", () => {
  it("resizes the tmux pane when the sidebar toggles, keeping the terminal and its scroll-back", async () => {
    const frame = (replay: string) => ({ replay, alternate_screen: false, mouse_reporting: false });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "attach_terminal") return { key: "feature:claude", generation: 1, initial: frame("live") };
      if (command === "terminal_history") return {
        replay: Array.from({ length: 80 }, (_, i) => `line-${i}`).join("\r\n"), earlier_lines: 56, alternate_screen: false,
      };
      if (command === "session_sidebar") return sidebarView();
      return undefined;
    });
    const target = { project_id: "project", feature_id: "feature", session_id: "claude" };
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <div className="session-split">
          <div className="session-split-main"><TerminalPane target={target} /></div>
          <SessionSidebar target={target} onReusePrompt={vi.fn()} onPrTriage={vi.fn()} onSupervisedEdits={vi.fn()} onFreshSession={vi.fn()} />
        </div>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(calls("attach_terminal")).toHaveLength(1));
    await screen.findByRole("heading", { name: "Claude Sidebar" });

    // Read earlier output, then hide the sidebar: the layout widens the terminal.
    const surface = document.querySelector(".term-surface") as HTMLElement;
    await act(async () => { fireEvent.wheel(surface, { deltaY: -48 }); });
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("Viewing earlier output"));
    const refreshesBefore = calls("terminal_refresh").length;

    fireEvent.click(screen.getByRole("button", { name: "Hide agent sidebar" }));
    relayout(132);
    expect(calls("resize_terminal")).toEqual([["resize_terminal", { key: "feature:claude", size: { cols: 132, rows: 24 } }]]);

    // Showing it again narrows the pane back.
    fireEvent.click(screen.getByRole("button", { name: "Show agent sidebar" }));
    relayout(100);
    expect(calls("resize_terminal")[1]).toEqual(["resize_terminal", { key: "feature:claude", size: { cols: 100, rows: 24 } }]);

    // Still reading history: new output is set aside, not drawn over it.
    expect(screen.getByRole("status").textContent).toContain("Viewing earlier output");
    act(() => emit(frame("newer")));
    expect(screen.getByRole("status").textContent).toContain("New output below");
    expect(calls("terminal_refresh").length).toBe(refreshesBefore);
    expect(calls("attach_terminal")).toHaveLength(1);
    expect(calls("detach_terminal")).toHaveLength(0);
    client.clear();
  });
});

describe("agent tab in the app", () => {
  function snapshot(): WorkspaceSnapshot {
    return {
      projects: [{
        id: "project", name: "demo", repo: "/home/me/demo", is_git: true, collapsed: false,
        features: [{
          id: "feature", name: "my-feat", branch: "my-feat", workdir: "/home/me/demo/.worktrees/my-feat",
          is_worktree: true, status: "idle", agent: "claude", mode: "vibe", collapsed: false,
          sessions: [
            { id: "claude", kind: "claude", label: "Claude 1", tmux_window: "claude" },
            { id: "shell", kind: "terminal", label: "Shell", tmux_window: "shell" },
          ],
        }],
      }],
      snapshot_at: "2026-10-07T00:00:00Z",
      stopped_session_ids: [],
      sidebar: { projects: {}, features: {}, sessions: {} },
    } as unknown as WorkspaceSnapshot;
  }

  it("reuses the last prompt into the draft and keeps the draft and terminal across toggles", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      switch (command) {
        case "get_snapshot": return snapshot();
        case "supported_harnesses": return [{ slug: "claude", display_name: "Claude" }];
        case "supported_modes": return [{ slug: "vibe", display_name: "Vibe", description: "" }];
        case "plan_snapshot": return { active: null, precall: null };
        case "session_sidebar": return sidebarView();
        case "attach_terminal": return { key: "feature:claude", generation: 1, initial: { replay: "", alternate_screen: false, mouse_reporting: false } };
        case "terminal_history": return {
          replay: Array.from({ length: 80 }, (_, i) => `line-${i}`).join("\r\n"), earlier_lines: 56, alternate_screen: false,
        };
        default: return undefined;
      }
    });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><App /></QueryClientProvider>);
    fireEvent.click(await screen.findByRole("button", { name: "my-feat", exact: true }));
    const sidebar = await screen.findByRole("complementary", { name: "Claude Sidebar" });

    const draft = screen.getByRole("textbox", { name: "Draft prompt" }) as HTMLTextAreaElement;
    fireEvent.change(draft, { target: { value: "keep me" } });
    fireEvent.click(within(sidebar).getByRole("button", { name: "Reuse" }));
    await waitFor(() => expect(draft.value).toBe(`keep me\n\n${PROMPT}`));

    const attaches = calls("attach_terminal").length;
    fireEvent.click(within(sidebar).getByRole("button", { name: "Hide agent sidebar" }));
    expect(screen.getByRole("button", { name: "Show agent sidebar" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Show agent sidebar" }));
    await screen.findByRole("complementary", { name: "Claude Sidebar" });
    expect((screen.getByRole("textbox", { name: "Draft prompt" }) as HTMLTextAreaElement).value).toBe(`keep me\n\n${PROMPT}`);
    expect(calls("attach_terminal").length).toBe(attaches);
    expect(calls("detach_terminal")).toHaveLength(0);

    // Read history before changing both sidebars; reflow must not jump to live output.
    await act(async () => { fireEvent.wheel(document.querySelector(".term-surface")!, { deltaY: -48 }); });
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("Viewing earlier output"));
    // Both panels resize the existing terminal, independently, in every combination.
    fireEvent.click(screen.getByRole("button", { name: "Hide projects sidebar" }));
    relayout(140);
    expect(screen.getByRole("button", { name: "Hide agent sidebar" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Hide agent sidebar" }));
    relayout(172);
    expect(screen.getByRole("button", { name: "Show projects sidebar" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Show projects sidebar" }));
    relayout(132);
    expect(screen.getByRole("button", { name: "Show agent sidebar" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Show agent sidebar" }));
    relayout(100);
    expect(calls("resize_terminal").slice(-4).map(([, args]) =>
      (args as { size: { cols: number } }).size.cols)).toEqual([140, 172, 132, 100]);
    expect((screen.getByRole("textbox", { name: "Draft prompt" }) as HTMLTextAreaElement).value).toBe(`keep me\n\n${PROMPT}`);
    expect(calls("attach_terminal")).toHaveLength(attaches);
    expect(calls("detach_terminal")).toHaveLength(0);

    expect(screen.getByRole("status").textContent).toContain("Viewing earlier output");
    // A plain terminal tab has no agent sidebar.
    fireEvent.click(screen.getByRole("tab", { name: /Shell/ }));
    await waitFor(() => expect(screen.queryByRole("complementary", { name: "Claude Sidebar" })).toBeNull());
    expect(screen.queryByRole("button", { name: "Show agent sidebar" })).toBeNull();
    client.clear();
  });
});
