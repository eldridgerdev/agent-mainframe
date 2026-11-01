import type { TreeRow } from "./diffTree";
import { ancestorDirs } from "./diffTree";

/** What a key does in the standalone Changes view (the TUI's `handlers/diff.rs`, plus n/p). */
export type DiffCommand =
  | "next-row" | "prev-row" | "first-file" | "last-file" | "next-hunk" | "prev-hunk"
  | "layout" | "refresh" | "whitespace" | "amf" | "fold" | "fold-all" | "collapse" | "expand" | "help";

/** The documented bindings, in one place so the help text cannot drift from the handler. */
export const DIFF_KEYS: { key: string; command: DiffCommand; help: string; tui: boolean }[] = [
  { key: "j", command: "next-row", help: "Next row in the file tree", tui: true },
  { key: "k", command: "prev-row", help: "Previous row in the file tree", tui: true },
  { key: "g", command: "first-file", help: "First file", tui: true },
  { key: "G", command: "last-file", help: "Last file", tui: true },
  { key: "n", command: "next-hunk", help: "Next hunk (desktop only)", tui: false },
  { key: "p", command: "prev-hunk", help: "Previous hunk (desktop only)", tui: false },
  { key: "z", command: "fold", help: "Fold or unfold the folder (or AMF group) under the cursor", tui: true },
  { key: "Z", command: "fold-all", help: "Fold every folder, or unfold all", tui: true },
  { key: "h", command: "collapse", help: "Collapse the folder, or move to the parent folder", tui: true },
  { key: "l", command: "expand", help: "Expand the folder", tui: true },
  { key: ".", command: "amf", help: "Show or hide the AMF files group", tui: true },
  { key: "v", command: "layout", help: "Switch unified / side by side (remembered)", tui: true },
  { key: "r", command: "refresh", help: "Refresh the diff", tui: true },
  { key: "W", command: "whitespace", help: "Ignore whitespace on or off", tui: true },
  { key: "?", command: "help", help: "Show or hide this list", tui: false },
];

const FIELD = "input, textarea, select, [contenteditable]:not([contenteditable='false']), [role='textbox'], .term-frame";

function visible(element: Element): boolean {
  for (let e: Element | null = element; e; e = e.parentElement) {
    if (e.hasAttribute("hidden") || window.getComputedStyle(e).display === "none") return false;
  }
  return true;
}

/**
 * The command a key press means for this panel, or null. Never for a
 * modified key, a repeat of a toggle, a text field, or when another dialog is
 * in front of the panel's own.
 */
export function diffKeyCommand(event: KeyboardEvent, ownDialog: Element | null | undefined): DiffCommand | null {
  if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey || event.isComposing) return null;
  const entry = DIFF_KEYS.find((item) => item.key === event.key);
  if (!entry) return null;
  if (event.repeat && entry.command !== "next-row" && entry.command !== "prev-row") return null;
  const target = event.target instanceof Element ? event.target : null;
  if (target?.closest(FIELD)) return null;
  if (!ownDialog || !visible(ownDialog)) return null;
  const dialogs = [...document.querySelectorAll("[role='dialog']")].filter(visible);
  if (dialogs[dialogs.length - 1] !== ownDialog) return null;
  // A key aimed at some other dialog's subtree is not ours even if it nests.
  if (target && !ownDialog.contains(target) && target !== document.body) return null;
  return entry.command;
}

export const rowKey = (row: TreeRow): string =>
  row.kind === "dir" ? `dir:${row.key}` : row.kind === "file" ? `file:${row.file.path}` : "group";

/** The cursor to use: the remembered row if it is still visible, else the selected file's, else the first. */
export function effectiveCursor(rows: TreeRow[], cursor: string | null, selectedPath: string | undefined): string | null {
  const keys = rows.map(rowKey);
  if (cursor && keys.includes(cursor)) return cursor;
  // The remembered row was folded away: land on its nearest visible folder.
  for (const origin of [cursor?.replace(/^(dir|file):/, ""), selectedPath]) {
    if (!origin || origin === "group") continue;
    const parent = ancestorDirs(origin + (cursor?.startsWith("dir:") && origin === cursor.slice(4) ? "/x" : "")).reverse().find((dir) => keys.includes(`dir:${dir}`));
    if (parent) return `dir:${parent}`;
  }
  if (selectedPath && keys.includes(`file:${selectedPath}`)) return `file:${selectedPath}`;
  return keys[0] ?? null;
}

export function moveCursor(rows: TreeRow[], cursor: string | null, delta: 1 | -1): string | null {
  const keys = rows.map(rowKey);
  if (keys.length === 0) return null;
  const at = cursor ? keys.indexOf(cursor) : -1;
  return keys[Math.min(keys.length - 1, Math.max(0, at < 0 ? 0 : at + delta))];
}

export function edgeFile(rows: TreeRow[], last: boolean): string | null {
  const files = rows.filter((row) => row.kind === "file");
  const row = last ? files[files.length - 1] : files[0];
  return row ? rowKey(row) : null;
}

export interface FoldState { collapsed: ReadonlySet<string>; amfChoice: boolean | null }
export interface FoldResult extends FoldState { cursor: string | null }

/** `z` / `h` / `l` on the row under the cursor. Files fold nothing; `h` on one climbs to its folder. */
export function applyFold(
  command: "fold" | "collapse" | "expand", rows: TreeRow[], cursor: string | null, state: FoldState,
): FoldResult {
  const row = rows.find((item) => rowKey(item) === cursor);
  const unchanged: FoldResult = { ...state, cursor };
  if (!row) return unchanged;
  const setDir = (key: string, open: boolean): ReadonlySet<string> => {
    const next = new Set(state.collapsed);
    if (open) next.delete(key); else next.add(key);
    return next;
  };
  if (row.kind === "group") {
    if (command === "fold") return { ...state, amfChoice: !row.open, cursor };
    if (command === "collapse" && row.open) return { ...state, amfChoice: false, cursor };
    if (command === "expand" && !row.open) return { ...state, amfChoice: true, cursor };
    return unchanged;
  }
  if (row.kind === "dir") {
    if (command === "fold") return { ...state, collapsed: setDir(row.key, !row.open), cursor };
    if (command === "expand") return row.open ? unchanged : { ...state, collapsed: setDir(row.key, true), cursor };
    if (row.open) return { ...state, collapsed: setDir(row.key, false), cursor };
  }
  if (command === "collapse") {
    if (row.kind === "file" && row.file.amf_generated) {
      return rows.some((item) => item.kind === "group") ? { ...unchanged, cursor: "group" } : unchanged;
    }
    const parent = ancestorDirs(row.kind === "dir" ? row.key : row.file.path).pop();
    if (parent && rows.some((item) => item.kind === "dir" && item.key === parent)) return { ...unchanged, cursor: `dir:${parent}` };
  }
  return unchanged;
}
