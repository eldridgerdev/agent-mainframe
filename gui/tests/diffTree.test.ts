import { expect, it } from "vitest";
import type { DiffFile } from "../src/api";
import { buildRows, selectFile, toggleAllFolds, ancestorDirs } from "../src/diffTree";

const f = (path: string, amf = false): DiffFile => ({ path, old_path: null, status: "modified", additions: 1, deletions: 0, is_binary: false, hunks: [], patch: "", amf_generated: amf });
const files = [f("src/b.rs"), f("README.md"), f("src/app/a.rs"), f(".amf/x.txt", true), f("src/app/z.rs"), f("AMF_PLAN.md", true)];
const names = (rows: ReturnType<typeof buildRows>) => rows.map((r) => r.kind === "file" ? `f:${r.file.path}` : r.kind === "dir" ? `d:${r.key}${r.open ? "" : "(folded)"}` : `g:${r.count}${r.open ? "" : "(folded)"}`);

it("lists ancestors shallowest first", () => {
  expect(ancestorDirs("a/b/c.rs")).toEqual(["a", "a/b"]);
  expect(ancestorDirs("c.rs")).toEqual([]);
});

it("builds a tree with directories before files and AMF files in a folded group", () => {
  expect(names(buildRows(files, { collapsed: new Set(), amfChoice: null, filter: "" }))).toEqual([
    "d:src", "d:src/app", "f:src/app/a.rs", "f:src/app/z.rs", "f:src/b.rs", "f:README.md", "g:2(folded)",
  ]);
});

it("shows the AMF group when toggled, and open by default when it is all there is", () => {
  expect(names(buildRows(files, { collapsed: new Set(), amfChoice: true, filter: "" })).slice(-3)).toEqual(["g:2", "f:.amf/x.txt", "f:AMF_PLAN.md"]);
  expect(names(buildRows([f(".amf/x.txt", true)], { collapsed: new Set(), amfChoice: null, filter: "" }))).toEqual(["g:1", "f:.amf/x.txt"]);
});

it("folds a directory with its subtree and keeps the others", () => {
  expect(names(buildRows(files, { collapsed: new Set(["src"]), amfChoice: null, filter: "" }))).toEqual(["d:src(folded)", "f:README.md", "g:2(folded)"]);
  expect(names(buildRows(files, { collapsed: new Set(["src/app"]), amfChoice: null, filter: "" }))).toContain("d:src/app(folded)");
});

it("filters across folded directories and the folded AMF group, then restores folds", () => {
  const collapsed = new Set(["src", "src/app"]);
  expect(names(buildRows(files, { collapsed, amfChoice: null, filter: "z.rs" }))).toEqual(["d:src", "d:src/app", "f:src/app/z.rs"]);
  expect(names(buildRows(files, { collapsed, amfChoice: false, filter: "X.TXT" }))).toEqual(["g:1", "f:.amf/x.txt"]);
  expect(names(buildRows(files, { collapsed, amfChoice: null, filter: "nothing" }))).toEqual([]);
  expect(names(buildRows(files, { collapsed, amfChoice: null, filter: "" }))[0]).toBe("d:src(folded)");
});

it("Z folds every directory, and unfolds when any is folded", () => {
  const folded = toggleAllFolds(files, new Set());
  expect([...folded].sort()).toEqual(["src", "src/app"]);
  expect(toggleAllFolds(files, new Set(["src"])).size).toBe(0);
});

it("keeps selection by path, including inside folded directories and across refresh", () => {
  expect(selectFile(files, "src/app/z.rs", "")?.path).toBe("src/app/z.rs");
  expect(selectFile(files, null, "")?.path).toBe("src/app/a.rs");
  expect(selectFile(files, "gone.rs", "")?.path).toBe("src/app/a.rs");
  expect(selectFile(files, ".amf/x.txt", "")?.path).toBe(".amf/x.txt");
  expect(selectFile(files, "src/b.rs", "app")?.path).toBe("src/app/a.rs");
  expect(selectFile(files, "src/b.rs", "zzz")).toBeUndefined();
  const refreshed = files.map((x) => ({ ...x, additions: 9 }));
  expect(selectFile(refreshed, "src/b.rs", "")?.additions).toBe(9);
});
