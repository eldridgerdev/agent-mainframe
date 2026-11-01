import type { DiffFile } from "./api";

/** One visible row of the changed-files tree. Directory keys are slash paths. */
export type TreeRow =
  | { kind: "dir"; key: string; name: string; depth: number; open: boolean; files: number }
  | { kind: "file"; file: DiffFile; name: string; depth: number }
  | { kind: "group"; count: number; open: boolean };

/** Every ancestor directory of `path`, shallowest first (the TUI's `ancestor_dirs`). */
export function ancestorDirs(path: string): string[] {
  const parts = path.split("/").slice(0, -1);
  return parts.map((_, i) => parts.slice(0, i + 1).join("/"));
}

/** Directories before files at each level, then by name: contiguous subtrees. */
function compare(a: string, b: string): number {
  const x = a.split("/"), y = b.split("/");
  for (let i = 0; i < Math.min(x.length, y.length); i++) {
    if (x[i] === y[i]) continue;
    const xDir = i < x.length - 1, yDir = i < y.length - 1;
    if (xDir !== yDir) return xDir ? -1 : 1;
    return x[i] < y[i] ? -1 : 1;
  }
  return x.length - y.length;
}

export const matchesFilter = (file: DiffFile, filter: string) =>
  file.path.toLowerCase().includes(filter.toLowerCase());

/** Files in tree order: the reviewer's own first, then AMF's own beneath them. */
export function orderFiles(files: DiffFile[]): { user: DiffFile[]; amf: DiffFile[] } {
  const sorted = [...files].sort((a, b) => compare(a.path, b.path));
  return { user: sorted.filter((f) => !f.amf_generated), amf: sorted.filter((f) => f.amf_generated) };
}

/**
 * The AMF group defaults to folded beside files of the user's, and to open
 * when it is all there is (folding would only hide the list), as in the TUI.
 */
export const amfGroupOpen = (choice: boolean | null, userCount: number) => choice ?? userCount === 0;

/**
 * Build the visible rows. A non-empty filter searches every file, including
 * those inside folded directories or the folded AMF group, and shows its
 * matches with their ancestors open; folds apply again once it is cleared.
 */
export function buildRows(files: DiffFile[], opts: { collapsed: ReadonlySet<string>; amfChoice: boolean | null; filter: string }): TreeRow[] {
  const filtering = opts.filter.trim() !== "";
  const { user, amf } = orderFiles(files.filter((f) => !filtering || matchesFilter(f, opts.filter)));
  const counts = new Map<string, number>();
  for (const f of user) for (const dir of ancestorDirs(f.path)) counts.set(dir, (counts.get(dir) ?? 0) + 1);
  const rows: TreeRow[] = [];
  const seen = new Set<string>();
  for (const f of user) {
    const ancestors = ancestorDirs(f.path);
    let hidden = false;
    ancestors.forEach((dir, depth) => {
      if (hidden) return;
      if (!seen.has(dir)) {
        seen.add(dir);
        const open = filtering || !opts.collapsed.has(dir);
        rows.push({ kind: "dir", key: dir, name: dir.split("/").pop()!, depth, open, files: counts.get(dir)! });
      }
      if (!filtering && opts.collapsed.has(dir)) hidden = true;
    });
    if (!hidden) rows.push({ kind: "file", file: f, name: f.path.split("/").pop()!, depth: ancestors.length });
  }
  if (amf.length > 0) {
    const open = filtering || amfGroupOpen(opts.amfChoice, user.length);
    rows.push({ kind: "group", count: amf.length, open });
    // AMF's files sit flat beneath the group row, whole path shown.
    if (open) for (const f of amf) rows.push({ kind: "file", file: f, name: f.path, depth: 1 });
  }
  return rows;
}

/** All directories of the user's files (the AMF group is not part of "the tree"). */
export const allDirs = (files: DiffFile[]): string[] =>
  [...new Set(files.filter((f) => !f.amf_generated).flatMap((f) => ancestorDirs(f.path)))];

/** Fold every directory, or unfold all when anything is already folded. */
export const toggleAllFolds = (files: DiffFile[], collapsed: ReadonlySet<string>): Set<string> =>
  collapsed.size > 0 ? new Set() : new Set(allDirs(files));

/**
 * The file shown: the chosen path while it still exists in the (filtered)
 * set, else the first one in tree order. Keyed by path so a refresh keeps it,
 * even when its row is folded away.
 */
export function selectFile(files: DiffFile[], path: string | null, filter: string): DiffFile | undefined {
  const visible = filter.trim() === "" ? files : files.filter((f) => matchesFilter(f, filter));
  const { user, amf } = orderFiles(visible);
  const ordered = [...user, ...amf];
  return ordered.find((f) => f.path === path) ?? ordered[0];
}
