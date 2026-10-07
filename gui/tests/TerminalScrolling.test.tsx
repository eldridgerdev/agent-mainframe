// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import TerminalPane from "../src/TerminalPane";
import { WheelAccumulator, scrollKeyAction } from "../src/terminalScroll";

// A small stand-in for xterm.js that models what scrolling depends on: a
// buffer whose written lines beyond the screen become scrollback, a viewport
// position, and the scroll/data/key hooks TerminalPane registers. Race tests
// defer both rendering and completion; reset deliberately leaves writes queued,
// matching xterm's behavior.
const fake = vi.hoisted(() => {
  class FakeTerminal {
    static current: FakeTerminal;
    cols = 80;
    rows = 24;
    element: HTMLElement | undefined;
    buffer = { active: { viewportY: 0, baseY: 0 } };
    lines = 0;
    screen: string[] = [];
    resets = 0;
    deferWrites = false;
    writes: { data: string; callback?: () => void }[] = [];
    keyHandler: (event: KeyboardEvent) => boolean = () => true;
    dataHandlers: ((data: string) => void)[] = [];
    scrollHandlers: ((position: number) => void)[] = [];
    constructor() { FakeTerminal.current = this; }
    loadAddon() {}
    refresh() {}
    dispose() {}
    open(parent: HTMLElement) {
      this.element = document.createElement("div");
      this.element.className = "xterm";
      parent.appendChild(this.element);
    }
    reset() {
      this.resets += 1;
      this.lines = 0;
      this.screen = [];
      this.buffer.active.viewportY = 0;
      this.buffer.active.baseY = 0;
    }
    write(data: string, callback?: () => void) {
      if (this.deferWrites) {
        this.writes.push({ data, callback });
        return;
      }
      this.applyWrite(data, callback);
    }
    finishWrite() {
      const next = this.writes.shift();
      if (!next) throw new Error("No pending terminal write");
      this.applyWrite(next.data, next.callback);
    }
    applyWrite(data: string, callback?: () => void) {
      this.screen.push(data);
      this.lines += data.split("\r\n").length;
      this.buffer.active.baseY = Math.max(0, this.lines - this.rows);
      this.buffer.active.viewportY = this.buffer.active.baseY;
      this.fireScroll();
      callback?.();
    }
    scrollLines(amount: number) {
      const { baseY } = this.buffer.active;
      this.buffer.active.viewportY = Math.min(baseY, Math.max(0, this.buffer.active.viewportY + amount));
      this.fireScroll();
    }
    scrollToBottom() { this.scrollLines(this.buffer.active.baseY); }
    scrollToTop() { this.scrollLines(-this.buffer.active.baseY); }
    fireScroll() { this.scrollHandlers.forEach((handler) => handler(this.buffer.active.viewportY)); }
    onData(handler: (data: string) => void) {
      this.dataHandlers.push(handler);
      return { dispose: () => {} };
    }
    onResize() { return { dispose: () => {} }; }
    onScroll(handler: (position: number) => void) {
      this.scrollHandlers.push(handler);
      return { dispose: () => { this.scrollHandlers = this.scrollHandlers.filter((h) => h !== handler); } };
    }
    attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean) { this.keyHandler = handler; }
    type(data: string) { this.dataHandlers.forEach((handler) => handler(data)); }
  }
  return { FakeTerminal };
});
vi.mock("@xterm/xterm", () => ({ Terminal: fake.FakeTerminal }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit = vi.fn(); } }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const target = { project_id: "project", feature_id: "feature", session_id: "agent" };
const key = "feature:agent";
const frame = (replay: string, alternate_screen = false, mouse_reporting = false) =>
  ({ replay, alternate_screen, mouse_reporting });
const historyReplay = Array.from({ length: 100 }, (_, i) => `line-${i + 1}`).join("\r\n");
let emit: (payload: unknown) => void;
let history: unknown;

const term = () => fake.FakeTerminal.current;
const surface = () => document.querySelector(".term-surface") as HTMLElement;
const commands = () => vi.mocked(invoke).mock.calls.map(([command]) => command);
const key_ = (keyName: string, init: KeyboardEventInit = {}) =>
  term().keyHandler(new KeyboardEvent("keydown", { key: keyName, ...init }));

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  history = { replay: historyReplay, earlier_lines: 76, alternate_screen: false };
  vi.mocked(listen).mockImplementation(async (_event, handler) => {
    emit = (payload) => (handler as (event: { payload: unknown }) => void)({ payload });
    return () => {};
  });
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "attach_terminal") return { key, generation: 7, initial: frame("live screen") };
    if (command === "terminal_history") return history;
    if (command === "terminal_wheel") return true;
    return undefined;
  });
});
afterEach(() => { cleanup(); vi.clearAllMocks(); vi.unstubAllGlobals(); });

async function attach(initial = frame("live screen")) {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "attach_terminal") return { key, generation: 7, initial };
    if (command === "terminal_history") return history;
    if (command === "terminal_wheel") return true;
    return undefined;
  });
  const view = render(<TerminalPane target={target} />);
  await waitFor(() => expect(term().screen).toEqual([initial.replay]));
  return view;
}

async function wheelUp(deltaY = -48) {
  await act(async () => { fireEvent.wheel(surface(), { deltaY }); });
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

describe("terminal scroll-back", () => {
  it("waits for history bytes before resetting and replaying the newest live frame", async () => {
    await attach();
    term().deferWrites = true;
    await wheelUp();
    const resets = term().resets;
    act(() => {
      key_("Escape");
      emit(frame("older live frame"));
      emit(frame("newest live frame"));
    });
    expect(term().resets).toBe(resets);
    expect(term().writes.map(({ data }) => data)).toEqual([historyReplay]);
    act(() => term().finishWrite());
    expect(term().resets).toBe(resets + 1);
    expect(term().screen).toEqual([]);
    act(() => term().finishWrite());
    expect(term().screen).toEqual(["newest live frame"]);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("restores the last live frame when history is cancelled without new output", async () => {
    await attach();
    term().deferWrites = true;
    await wheelUp();
    act(() => key_("Escape"));
    act(() => term().finishWrite());
    act(() => term().finishWrite());
    expect(term().screen).toEqual(["live screen"]);
    expect(term().buffer.active).toEqual({ baseY: 0, viewportY: 0 });
  });

  it("waits for live write completion before resetting for history", async () => {
    await attach();
    term().deferWrites = true;
    act(() => emit(frame("pending live")));
    const resets = term().resets;
    await wheelUp();
    expect(term().resets).toBe(resets);
    act(() => term().finishWrite());
    act(() => term().finishWrite());
    expect(term().screen).toEqual([historyReplay]);
    expect(term().buffer.active.viewportY).toBe(73);
    expect(screen.getByRole("status").textContent).toContain("Viewing earlier output");
  });

  it.each(["resolve", "reject"] as const)("ignores a cancelled request that later %ss during another load", async (settlement) => {
    await attach();
    const first = deferred<unknown>();
    const second = deferred<unknown>();
    vi.mocked(invoke).mockImplementation((command) => {
      if (command === "terminal_history") return commands().filter((name) => name === command).length === 1
        ? first.promise : second.promise;
      return Promise.resolve();
    });
    await wheelUp();
    act(() => key_("Escape"));
    await wheelUp();
    await act(async () => {
      if (settlement === "resolve") first.resolve({ replay: "cancelled snapshot", earlier_lines: 10, alternate_screen: false });
      else first.reject(new Error("cancelled request failed"));
    });
    expect(term().screen).toEqual(["live screen"]);
    expect(screen.getByRole("status").textContent).toContain("Loading earlier output");
    await act(async () => second.resolve(history));
    expect(term().screen).toEqual([historyReplay]);
    expect(term().buffer.active.viewportY).toBe(73);
    expect(screen.getByRole("status").textContent).toContain("Viewing earlier output");
  });

  it("ignores a cancelled history write callback while the next request is loading", async () => {
    await attach();
    term().deferWrites = true;
    await wheelUp();
    act(() => key_("Escape"));
    const second = deferred<unknown>();
    vi.mocked(invoke).mockImplementation((command) => command === "terminal_history"
      ? second.promise : Promise.resolve());
    await wheelUp();
    act(() => term().finishWrite());
    expect(screen.getByRole("status").textContent).toContain("Loading earlier output");
    expect(term().buffer.active.viewportY).toBe(76);
    await act(async () => second.resolve({ ...history as object, replay: `${historyReplay}\r\nnew snapshot` }));
    act(() => term().finishWrite());
    expect(term().screen).toEqual([`${historyReplay}\r\nnew snapshot`]);
    expect(term().buffer.active.viewportY).toBe(74);
    expect(screen.getByRole("status").textContent).toContain("Viewing earlier output");
  });

  it("replaces cancelled history with a second snapshot resolved before the first write completes", async () => {
    await attach();
    term().deferWrites = true;
    await wheelUp();
    act(() => key_("Escape"));
    const secondReplay = `${historyReplay}\r\nsecond snapshot`;
    history = { replay: secondReplay, earlier_lines: 77, alternate_screen: false };
    await wheelUp();
    expect(term().writes.map(({ data }) => data)).toEqual([historyReplay]);
    act(() => term().finishWrite());
    expect(screen.getByRole("status").textContent).toContain("Loading earlier output");
    expect(term().writes.map(({ data }) => data)).toEqual([secondReplay]);
    act(() => term().finishWrite());
    expect(term().screen).toEqual([secondReplay]);
    expect(term().buffer.active.viewportY).toBe(74);
    expect(screen.getByRole("status").textContent).toContain("Viewing earlier output");
  });

  it("does not start a queued replay after unmounting with an in-flight write", async () => {
    const view = await attach();
    term().deferWrites = true;
    await wheelUp();
    act(() => key_("Escape"));
    const resets = term().resets;
    view.unmount();
    act(() => term().finishWrite());
    expect(term().resets).toBe(resets);
    expect(term().writes).toEqual([]);
    expect(invoke).toHaveBeenCalledWith("detach_terminal", { key, generation: 7 });
  });

  it("loads tmux history on wheel up, keeps the reader's place under new output, and jumps back to live", async () => {
    await attach();

    await wheelUp();

    expect(invoke).toHaveBeenCalledWith("terminal_history", { key });
    expect(term().screen).toEqual([historyReplay]);
    // 100 lines on a 24-row screen: 76 above it, and three wheel steps up.
    expect(term().buffer.active).toEqual({ baseY: 76, viewportY: 73 });
    expect(screen.getByRole("status").textContent).toContain("Viewing earlier output");

    const resetsWhileReading = term().resets;
    act(() => emit(frame("newer live screen")));
    act(() => emit(frame("newest live screen")));

    // Not redrawn underneath the reader, and still where they left it.
    expect(term().resets).toBe(resetsWhileReading);
    expect(term().buffer.active.viewportY).toBe(73);
    expect(screen.getByRole("status").textContent).toContain("New output below");

    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Jump to latest" })); });

    expect(term().screen).toEqual(["newest live screen"]);
    expect(screen.queryByRole("status")).toBeNull();
    expect(invoke).toHaveBeenCalledWith("terminal_refresh", { key });
    // Scrolling sent nothing to the program.
    expect(commands()).not.toContain("terminal_input");
    expect(commands()).not.toContain("terminal_wheel");

    // Live again: the next update draws immediately.
    act(() => emit(frame("after returning")));
    expect(term().screen).toEqual(["after returning"]);
  });

  it("scrolls with the keyboard and leaves on Esc without interrupting the agent", async () => {
    await attach();

    // From live, plain PageUp and Esc belong to the program.
    expect(key_("PageUp")).toBe(true);
    expect(key_("Escape")).toBe(true);
    expect(commands()).not.toContain("terminal_history");

    let handled!: boolean;
    await act(async () => { handled = key_("PageUp", { shiftKey: true }); });
    expect(handled).toBe(false);
    expect(term().buffer.active.viewportY).toBe(76 - 23);

    // While reading, plain PageUp/PageDown scroll by a page.
    act(() => { expect(key_("PageUp")).toBe(false); });
    expect(term().buffer.active.viewportY).toBe(76 - 46);
    act(() => { expect(key_("PageDown")).toBe(false); });
    expect(term().buffer.active.viewportY).toBe(76 - 23);
    act(() => { expect(key_("Home")).toBe(false); });
    expect(term().buffer.active.viewportY).toBe(0);

    await act(async () => { expect(key_("Escape")).toBe(false); });
    expect(screen.queryByRole("status")).toBeNull();
    expect(term().buffer.active.viewportY).toBe(term().buffer.active.baseY);
    expect(commands()).not.toContain("terminal_input");
  });

  it("returns to live when the reader scrolls to the bottom or starts typing", async () => {
    await attach();
    await wheelUp();
    act(() => emit(frame("arrived while reading")));

    // xterm scrolls its own scrollback while reading; reaching the end resumes.
    act(() => term().scrollLines(3));
    expect(screen.queryByRole("status")).toBeNull();
    expect(term().screen).toEqual(["arrived while reading"]);

    await wheelUp();
    expect(screen.getByRole("status").textContent).toContain("Viewing earlier output");
    act(() => term().type("y"));
    expect(screen.queryByRole("status")).toBeNull();
    expect(invoke).toHaveBeenCalledWith("terminal_input", { key, text: "y" });
  });

  it("forwards the wheel only to a full-screen program that asked for mouse reports", async () => {
    await attach(frame("opencode", true, true));

    await wheelUp();

    expect(invoke).toHaveBeenCalledWith("terminal_wheel", { key, direction: "up", steps: 3, col: 0, row: 0 });
    expect(commands()).not.toContain("terminal_history");

    act(() => emit(frame("less", true, false)));
    vi.mocked(invoke).mockClear();
    await wheelUp();

    expect(vi.mocked(invoke)).not.toHaveBeenCalled();
    expect(screen.getByRole("status").textContent).toContain("keeps no scrollback here");
  });

  it("says so when there is no earlier output, and stays live", async () => {
    history = { replay: "only screen", earlier_lines: 0, alternate_screen: false };
    await attach();

    await wheelUp();

    expect(screen.getByRole("status").textContent).toBe("There is no earlier output yet.");
    expect(term().screen).toEqual(["live screen"]);
    act(() => emit(frame("still live")));
    expect(term().screen).toEqual(["still live"]);
  });

  it("keeps focus in the composer when jumping to latest", async () => {
    render(<textarea aria-label="Compose prompt" />);
    await attach();
    const composer = screen.getByLabelText("Compose prompt");
    composer.focus();
    await wheelUp();

    const jump = screen.getByRole("button", { name: "Jump to latest" });
    // The press itself doesn't move focus off the draft.
    expect(fireEvent.mouseDown(jump)).toBe(false);
    await act(async () => { fireEvent.click(jump); });
    expect(document.activeElement).toBe(composer);
  });

  it("detaches cleanly when the tab closes while reading history", async () => {
    const view = await attach();
    await wheelUp();
    view.unmount();
    expect(invoke).toHaveBeenCalledWith("detach_terminal", { key, generation: 7 });
    expect(commands()).not.toContain("terminal_input");
  });
});

describe("scroll gesture helpers", () => {
  it("accumulates trackpad deltas into whole lines and resets on reversal", () => {
    const wheel = new WheelAccumulator();
    const pixels = (deltaY: number) => wheel.steps({ deltaY, deltaMode: 0 }, 16, 24);
    expect(pixels(-6)).toBe(0);
    expect(pixels(-6)).toBe(0);
    expect(pixels(-6)).toBe(-1);
    expect(pixels(4)).toBe(0);
    expect(wheel.steps({ deltaY: 3, deltaMode: 1 }, 16, 24)).toBe(3);
    expect(wheel.steps({ deltaY: -1, deltaMode: 2 }, 16, 24)).toBe(-24);
  });

  it("only scrolls on Shift keys from live, and on plain keys while reading", () => {
    const press = (key: string, init: KeyboardEventInit = {}) => new KeyboardEvent("keydown", { key, ...init });
    expect(scrollKeyAction(press("PageUp"), false)).toBeNull();
    expect(scrollKeyAction(press("PageUp", { shiftKey: true }), false)).toBe("pageUp");
    expect(scrollKeyAction(press("End", { shiftKey: true }), false)).toBe("latest");
    expect(scrollKeyAction(press("Escape"), false)).toBeNull();
    expect(scrollKeyAction(press("Escape"), true)).toBe("latest");
    expect(scrollKeyAction(press("PageDown"), true)).toBe("pageDown");
    expect(scrollKeyAction(press("Home"), true)).toBe("top");
    expect(scrollKeyAction(press("PageUp", { ctrlKey: true, shiftKey: true }), true)).toBeNull();
    expect(scrollKeyAction(press("a"), true)).toBeNull();
  });
});
