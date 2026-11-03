// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import {
  SHORTCUTS, SHORTCUT_GROUPS, formatBinding, setShortcutRegistryForTest, shortcutAria,
  shortcutCollisions, shortcutLabel, useShortcut, type DispatchShortcutId, type ShortcutDef,
} from "../src/shortcuts";
import KeyboardShortcutsDialog, { shortcutMatches } from "../src/KeyboardShortcutsDialog";
import HelpMenu, { GUI_DOCS_URL } from "../src/HelpMenu";
import { DIFF_KEYS } from "../src/diffKeys";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  setShortcutRegistryForTest(null);
});

const def = (id: string, keys: string[], scope: ShortcutDef["scope"], handler: ShortcutDef["handler"] = "dispatch"): ShortcutDef =>
  ({ id, keys, label: id, group: "General", scope, handler });

describe("registry", () => {
  it("has unique ids and no key collisions in overlapping scopes", () => {
    expect(shortcutCollisions()).toEqual([]);
  });

  it("documents every Changes key from the list its panel dispatches on", () => {
    for (const entry of DIFF_KEYS) {
      expect(SHORTCUTS.find((item) => item.id === `changes.${entry.command}`)?.keys).toEqual([entry.key]);
    }
  });

  it("puts every entry in a displayed group", () => {
    for (const entry of SHORTCUTS) expect(SHORTCUT_GROUPS).toContain(entry.group);
  });

  it("flags equal keys and prefixes in overlapping scopes only", () => {
    // Same key, same scope.
    expect(shortcutCollisions([def("a", ["?"], "workspace"), def("b", ["Shift+?"], "workspace")])).toHaveLength(1);
    // An ancestor scope collides with its children.
    expect(shortcutCollisions([def("a", ["Escape"], "dialog"), def("b", ["Escape"], "changes")])).toHaveLength(1);
    expect(shortcutCollisions([def("a", ["Mod+K"], "global"), def("b", ["Ctrl+K"], "terminal")])).toHaveLength(1);
    // Mod is Meta on macOS only.
    expect(shortcutCollisions([def("a", ["Mod+K"], "workspace"), def("b", ["Meta+K"], "workspace")]))
      .toEqual(["a (Mod+K) and b (Meta+K) collide on macOS"]);
    // A leader chord and a sequence that starts with it.
    expect(shortcutCollisions([def("a", ["Ctrl+Space"], "workspace"), def("b", ["Ctrl+Space G"], "workspace")])).toHaveLength(1);
    // Case matters for bare letters (TUI-style g / G).
    expect(shortcutCollisions([def("a", ["g"], "workspace"), def("b", ["G"], "workspace")])).toEqual([]);
    // Sibling scopes are separated by focus.
    expect(shortcutCollisions([def("a", ["Escape"], "sidebar-filter"), def("b", ["Escape"], "terminal")])).toEqual([]);
    expect(shortcutCollisions([def("a", ["?"], "workspace"), def("a", ["!"], "dialog")])).toEqual(["duplicate id a"]);
  });

  it("formats keys per platform", () => {
    expect(shortcutLabel("sidebar.projects.toggle", false)).toBe("Alt+Shift+P");
    expect(shortcutLabel("sidebar.projects.toggle", true)).toBe("Option+Shift+P");
    expect(shortcutLabel("composer.send", false)).toBe("Ctrl+Enter");
    expect(shortcutLabel("composer.send", true)).toBe("Cmd+Enter");
    expect(shortcutAria("composer.send", true)).toBe("Meta+Enter");
    expect(shortcutAria("menu.move", false)).toBe("ArrowDown ArrowUp");
    expect(formatBinding("Ctrl+Space G", false)).toBe("Ctrl+Space then G");
  });
});

function Bound({ id, handler }: { id: string; handler: (event: KeyboardEvent) => boolean | void }) {
  useShortcut(id as DispatchShortcutId, handler);
  return <input aria-label="Field" />;
}

describe("dispatcher", () => {
  it("fires workspace keys outside fields and dialogs, once per press", () => {
    const onHelp = vi.fn();
    render(<Bound id="help.shortcuts" handler={onHelp} />);
    expect(fireEvent.keyDown(document.body, { key: "?", shiftKey: true })).toBe(false);
    expect(onHelp).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(document.body, { key: "?", shiftKey: true, repeat: true });
    fireEvent.keyDown(screen.getByLabelText("Field"), { key: "?" });
    fireEvent.keyDown(document.body, { key: "?", ctrlKey: true });
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    document.body.appendChild(dialog);
    fireEvent.keyDown(document.body, { key: "?" });
    dialog.remove();
    expect(onHelp).toHaveBeenCalledTimes(1);
  });

  it("lets a declined key through", () => {
    render(<Bound id="help.shortcuts" handler={() => false} />);
    expect(fireEvent.keyDown(document.body, { key: "?" })).toBe(true);
  });

  it("runs leader sequences and drops a broken one", () => {
    setShortcutRegistryForTest([def("leader.go", ["Ctrl+Space G"], "workspace"), def("plain.g", ["g"], "workspace")]);
    const go = vi.fn();
    const plain = vi.fn();
    render(<><Bound id="leader.go" handler={go} /><Bound id="plain.g" handler={plain} /></>);
    expect(fireEvent.keyDown(document.body, { key: " ", ctrlKey: true })).toBe(false);
    fireEvent.keyDown(document.body, { key: "G", code: "KeyG" });
    expect(go).toHaveBeenCalledTimes(1);
    expect(plain).not.toHaveBeenCalled();
    // A key that continues no sequence is a fresh press.
    fireEvent.keyDown(document.body, { key: " ", ctrlKey: true });
    fireEvent.keyDown(document.body, { key: "g" });
    expect(go).toHaveBeenCalledTimes(1);
    expect(plain).toHaveBeenCalledTimes(1);
  });
});

describe("Keyboard shortcuts dialog", () => {
  it("lists every entry under its group", () => {
    render(<KeyboardShortcutsDialog onClose={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
    for (const group of SHORTCUT_GROUPS) expect(within(dialog).getByRole("heading", { name: group })).toBeTruthy();
    expect(dialog.querySelectorAll("[data-shortcut-id]")).toHaveLength(SHORTCUTS.length);
    const toggle = dialog.querySelector("[data-shortcut-id='sidebar.projects.toggle']")!;
    expect(toggle.textContent).toContain("Show or hide the projects sidebar");
    expect(Array.from(toggle.querySelectorAll("kbd"), (kbd) => kbd.textContent)).toEqual(["Alt", "Shift", "P"]);
  });

  it("filters by action, group and keys", () => {
    render(<KeyboardShortcutsDialog onClose={vi.fn()} />);
    const filter = screen.getByPlaceholderText("Filter shortcuts");
    fireEvent.change(filter, { target: { value: "hide the projects" } });
    expect(Array.from(document.querySelectorAll("[data-shortcut-id]"), (row) => row.getAttribute("data-shortcut-id")))
      .toEqual(["sidebar.projects.toggle"]);
    expect(screen.queryByRole("heading", { name: "Terminal" })).toBeNull();
    fireEvent.change(filter, { target: { value: "no such key" } });
    expect(screen.getByText(/No shortcut matches/)).toBeTruthy();
    expect(shortcutMatches(SHORTCUTS.find((item) => item.id === "composer.send")!, "ctrl enter", false)).toBe(true);
    expect(shortcutMatches(SHORTCUTS.find((item) => item.id === "composer.send")!, "terminal", false)).toBe(false);
  });

  it("closes on Escape", () => {
    const onClose = vi.fn();
    render(<KeyboardShortcutsDialog onClose={onClose} />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });
});

describe("Help menu", () => {
  it("offers shortcuts, docs and the first-run steps", async () => {
    const onShortcuts = vi.fn();
    const onFirstRun = vi.fn();
    const onError = vi.fn();
    vi.mocked(invoke).mockResolvedValue(undefined);
    render(<HelpMenu onShortcuts={onShortcuts} onFirstRun={onFirstRun} onError={onError} />);
    const open = () => fireEvent.click(screen.getByRole("button", { name: "Help" }));

    open();
    const shortcuts = screen.getByRole("menuitem", { name: "Keyboard shortcuts" });
    // The hint comes from the registry, the same entry the dispatcher binds.
    expect(shortcuts.querySelector(".menu-item-shortcut")?.textContent).toBe(shortcutLabel("help.shortcuts"));
    expect(shortcuts.getAttribute("aria-keyshortcuts")).toBe("?");
    fireEvent.click(shortcuts);
    expect(onShortcuts).toHaveBeenCalled();

    open();
    fireEvent.click(screen.getByRole("menuitem", { name: "Documentation" }));
    expect(invoke).toHaveBeenCalledWith("screenshots_open_browser", { url: GUI_DOCS_URL });

    open();
    fireEvent.click(screen.getByRole("menuitem", { name: "Show first-run steps again" }));
    expect(onFirstRun).toHaveBeenCalled();
    expect(onError).not.toHaveBeenCalled();
  });

  it("reports a docs link that cannot open", async () => {
    const onError = vi.fn();
    vi.mocked(invoke).mockRejectedValue(new Error("no browser"));
    render(<HelpMenu onShortcuts={vi.fn()} onFirstRun={vi.fn()} onError={onError} />);
    fireEvent.click(screen.getByRole("button", { name: "Help" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Documentation" }));
    await vi.waitFor(() => expect(onError).toHaveBeenCalled());
  });
});
