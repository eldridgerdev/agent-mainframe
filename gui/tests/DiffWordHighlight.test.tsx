// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import DiffPanel from "../src/DiffPanel";
import { SyntaxCode, splitCode } from "../src/SyntaxCode";
import type { DiffLine, DiffView } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
const target = { project_id: "project", feature_id: "feature" };

const removed: DiffLine = { kind: "removed", text: "-let x = foo(1);", old_line: 1, new_line: null, emphasis: [[13, 14]],
  syntax: [[null, "-"], ["keyword", "let"], [null, " x = "], ["function", "foo"], [null, "(1);"]] };
const added: DiffLine = { kind: "added", text: "+let x = foo(2);", old_line: null, new_line: 1, emphasis: [[13, 14]],
  syntax: [[null, "+"], ["keyword", "let"], [null, " x = "], ["function", "foo"], [null, "(2);"]] };

function viewWith(lines: DiffLine[]): DiffView {
  return { target, feature_name: "F", branch: "b", base_ref: "main", base_commit: "abc", commit: null, commits: [], commits_error: null,
    total_additions: 1, total_deletions: 1,
    files: [{ path: "a.rs", old_path: null, status: "modified", additions: 1, deletions: 1, is_binary: false, patch: "",
      hunks: [{ header: "@@ -1 +1 @@", lines }] }] } as DiffView;
}
function mount(view: DiffView) {
  vi.mocked(invoke).mockResolvedValue(view);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><DiffPanel target={target} onClose={vi.fn()} /></QueryClientProvider>);
}

it("splits on UTF-16 code units so emoji, CJK and combining text land exactly", () => {
  const text = "+名前 🦀 é end";
  const crab = text.indexOf("🦀");
  expect(splitCode(text, null, [[crab, crab + 2]]).filter(p => p.changed).map(p => p.part)).toEqual(["🦀"]);
  const accent = text.indexOf("́");
  expect(splitCode(text, null, [[accent - 1, accent + 1]]).filter(p => p.changed).map(p => p.part)).toEqual(["é"]);
  expect(splitCode(text, null, [[1, 3]]).filter(p => p.changed).map(p => p.part)).toEqual(["名前"]);
  expect(splitCode(text, null, [[1, 3]]).map(p => p.part).join("")).toBe(text);
});

it("cuts syntax runs at range edges and keeps every token's text", () => {
  const pieces = splitCode(removed.text, removed.syntax, [[4, 10]]);
  expect(pieces.map(p => p.part).join("")).toBe(removed.text);
  expect(pieces.filter(p => p.changed).map(p => p.part).join("")).toBe(removed.text.slice(4, 10));
  expect(pieces.find(p => p.part === "let")).toMatchObject({ token: "keyword" });
});

it("normalises malformed ranges instead of throwing", () => {
  const text = "+abcdef";
  const pieces = splitCode(text, null, [[5, 99], [2, 4], [3, 6], [4, 4], [-3, 1], [Number.NaN, 2]]);
  expect(pieces.map(p => p.part).join("")).toBe(text);
  expect(pieces.filter(p => p.changed).map(p => p.part).join("")).toBe(text.slice(0, 1) + text.slice(2, 7));
});

it("renders changed spans as marks with a change kind and keeps plain lines untouched", () => {
  const { container } = render(<SyntaxCode text="+ab" emphasis={[[2, 3]]} change="added" />);
  const mark = container.querySelector("mark")!;
  expect(mark.textContent).toBe("b");
  expect(mark.className).toContain("diff-word-added");
  cleanup();
  const plain = render(<SyntaxCode text="+ab" emphasis={[]} />);
  expect(plain.container.querySelector("mark")).toBeNull();
  expect(plain.container.textContent).toBe("+ab");
});

it("shows changed words in unified and split layouts together with syntax colours", async () => {
  mount(viewWith([removed, added]));
  const unified = await screen.findByRole("table", { name: "Unified hunk" });
  const marks = unified.querySelectorAll("mark");
  expect([...marks].map(m => m.textContent)).toEqual(["1", "2"]);
  expect(marks[0].className).toContain("diff-word-removed");
  expect(marks[1].className).toContain("diff-word-added");
  expect(unified.querySelectorAll(".syn-keyword").length).toBe(2);
  expect(within(unified).getAllByText("let").length).toBe(2);
  fireEvent.change(screen.getByRole("combobox", { name: "Layout" }), { target: { value: "split" } });
  const split = screen.getByRole("table", { name: "Side-by-side hunk" });
  expect([...split.querySelectorAll("mark")].map(m => m.textContent)).toEqual(["1", "2"]);
  expect(split.querySelectorAll(".syn-function").length).toBe(2);
});

it("falls back to line-level colouring when the backend sent no ranges (over budget)", async () => {
  mount(viewWith([{ ...removed, emphasis: undefined }, { ...added, emphasis: [] }]));
  const table = await screen.findByRole("table", { name: "Unified hunk" });
  expect(table.querySelector("mark")).toBeNull();
  expect(table.querySelector("tr.diff-removed")).toBeTruthy();
  expect(table.querySelector("tr.diff-added")).toBeTruthy();
  expect(table.textContent).toContain("-let x = foo(1);");
});
