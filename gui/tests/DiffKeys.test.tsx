// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import DiffPanel from "../src/DiffPanel";
import type { DiffFile, DiffView } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.clearAllMocks(); });
const target = { project_id: "project", feature_id: "feature" };
const file = (path: string, extra: Partial<DiffFile> = {}): DiffFile => ({
  path, old_path: null, status: "modified", additions: 1, deletions: 0, is_binary: false, patch: "",
  hunks: [{ header: `@@ ${path} one @@`, lines: [{ kind: "added", text: "+a", old_line: null, new_line: 1 }] },
    { header: `@@ ${path} two @@`, lines: [{ kind: "added", text: "+b", old_line: null, new_line: 9 }] }],
  ...extra,
} as DiffFile);
const view = (layout?: DiffView["layout"]): DiffView => ({
  target, feature_name: "Feature", branch: "feature", base_ref: "main", base_commit: "base123456", commit: null,
  commits: [], commits_error: null, total_additions: 4, total_deletions: 0, layout,
  files: [file("README.md"), file("src/a.rs"), file("src/b.rs"), file(".amf/note.md", { amf_generated: true })],
});
function mount(data = view()) {
  vi.mocked(invoke).mockImplementation(async (command) => command === "load_diff" ? data : undefined);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  render(<QueryClientProvider client={client}><DiffPanel target={target} onClose={vi.fn()} /></QueryClientProvider>);
  return screen.findByText("Hunk 1 of 2");
}
const press = (key: string, init: KeyboardEventInit = {}, on: EventTarget = document.body) =>
  fireEvent.keyDown(on, { key, ...init });
const selected = () => screen.getByRole("button", { pressed: true }).getAttribute("title");
const loads = () => vi.mocked(invoke).mock.calls.filter(([name]) => name === "load_diff").length;

it("j/k walk tree rows including folders, and g/G jump to the first and last file", async () => {
  await mount();
  expect(selected()).toBe("src/a.rs");           // folder src first, files after; first file in tree order
  press("k");                                    // the cursor starts on the file; k reaches its folder row
  expect(screen.getByRole("button", { name: "Folder src" }).getAttribute("aria-current")).toBe("true");
  press("j"); expect(selected()).toBe("src/a.rs");
  press("j"); expect(selected()).toBe("src/b.rs");
  press("k"); expect(selected()).toBe("src/a.rs");
  press("G"); expect(selected()).toBe("README.md");
  press("g"); expect(selected()).toBe("src/a.rs");
});

it("n/p step hunks and clamp at the ends", async () => {
  await mount();
  press("p"); expect(screen.getByText("Hunk 1 of 2")).toBeTruthy();
  press("n"); expect(screen.getByText("Hunk 2 of 2")).toBeTruthy();
  press("n"); expect(screen.getByText("Hunk 2 of 2")).toBeTruthy();
  press("p"); expect(screen.getByText("Hunk 1 of 2")).toBeTruthy();
});

it("z/h/l/Z fold the folder under the cursor and . toggles the AMF group", async () => {
  await mount();
  press("k"); // from the first file up to the src folder
  press("z");
  expect(screen.getByRole("button", { name: "Folder src" }).getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByTitle("src/b.rs")).toBeNull();
  press("l");
  expect(screen.getByTitle("src/b.rs")).toBeTruthy();
  press("h");
  expect(screen.queryByTitle("src/b.rs")).toBeNull();
  press("l");
  press("j"); press("j");                         // on src/b.rs
  press("h");                                     // a file climbs to its folder
  expect(screen.getByRole("button", { name: "Folder src" }).getAttribute("aria-current")).toBe("true");
  press("Z");
  expect(screen.getByRole("button", { name: "Folder src" }).getAttribute("aria-expanded")).toBe("false");
  press("Z");
  expect(screen.getByTitle("src/b.rs")).toBeTruthy();
  expect(screen.getByRole("button", { name: "AMF files group" }).getAttribute("aria-expanded")).toBe("false");
  press(".");
  expect(screen.getByRole("button", { name: "AMF files group" }).getAttribute("aria-expanded")).toBe("true");
  press(".");
  expect(screen.getByRole("button", { name: "AMF files group" }).getAttribute("aria-expanded")).toBe("false");
});

it("W and r reload with explicit requests", async () => {
  await mount();
  const before = loads();
  press("W");
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("load_diff", {
    target, options: { commit: null, base_ref: null, ignore_whitespace: true, context: "standard" } }));
  await screen.findByText("Hunk 1 of 2");
  expect((screen.getByRole("checkbox", { name: "Ignore whitespace" }) as HTMLInputElement).checked).toBe(true);
  const mid = loads();
  expect(mid).toBeGreaterThan(before);
  press("r");
  await waitFor(() => expect(loads()).toBe(mid + 1));
});

it("v toggles the layout and saves it in the TUI's format; a saved layout opens it", async () => {
  await mount(view("side-by-side"));
  expect((screen.getByRole("combobox", { name: "Layout" }) as HTMLSelectElement).value).toBe("split");
  press("v");
  expect((screen.getByRole("combobox", { name: "Layout" }) as HTMLSelectElement).value).toBe("unified");
  // Saves are chained on a promise, so the call lands a microtask later.
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenLastCalledWith("diff_layout_save", { layout: "unified" }));
  fireEvent.change(screen.getByRole("combobox", { name: "Layout" }), { target: { value: "split" } });
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenLastCalledWith("diff_layout_save", { layout: "side-by-side" }));
});

it("reports a layout save failure without losing the choice", async () => {
  await mount();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "diff_layout_save") throw { kind: "internal", message: "disk full" };
    return view();
  });
  press("v");
  expect(await screen.findByText(/Layout not saved: disk full/)).toBeTruthy();
  expect((screen.getByRole("combobox", { name: "Layout" }) as HTMLSelectElement).value).toBe("split");
});

it("keys are ignored in fields, with modifiers, under another dialog, and when hidden", async () => {
  await mount();
  const filter = screen.getByRole("textbox", { name: "Filter files" });
  press("G", {}, filter); expect(selected()).toBe("src/a.rs");
  press("v", {}, screen.getByRole("combobox", { name: "Layout" }));
  press("v", { ctrlKey: true }); press("v", { metaKey: true }); press("v", { altKey: true });
  press("W", { ctrlKey: true });
  expect(vi.mocked(invoke).mock.calls.some(([name]) => name === "diff_layout_save")).toBe(false);
  press("v", { repeat: true });
  expect(vi.mocked(invoke).mock.calls.some(([name]) => name === "diff_layout_save")).toBe(false);
  const other = document.createElement("div");
  other.setAttribute("role", "dialog");
  document.body.appendChild(other);
  press("G"); press("v");
  expect(selected()).toBe("src/a.rs");
  expect(vi.mocked(invoke).mock.calls.some(([name]) => name === "diff_layout_save")).toBe(false);
  other.remove();
  press("G"); expect(selected()).toBe("README.md");
});

it("documents every key in an accessible help list that ? toggles", async () => {
  await mount();
  const button = screen.getByRole("button", { name: "Keyboard shortcuts" });
  expect(button.getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(button);
  const help = screen.getByRole("region", { name: "Keyboard shortcuts" });
  for (const key of ["j", "k", "g", "G", "n", "p", "z", "Z", "h", "l", ".", "v", "r", "W", "?"]) {
    expect(help.textContent).toContain(key);
  }
  press("?");
  expect(screen.queryByRole("region", { name: "Keyboard shortcuts" })).toBeNull();
  press("?");
  expect(screen.getByRole("region", { name: "Keyboard shortcuts" })).toBeTruthy();
});
