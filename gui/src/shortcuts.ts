/**
 * Every keyboard shortcut the GUI has, in one typed registry.
 *
 * The Keyboard shortcuts dialog, menu hints (`MenuItem.shortcut`), tooltips
 * and `aria-keyshortcuts` all read from `SHORTCUTS`, so what the GUI says a
 * key does cannot drift from what it does.
 *
 * ## Adding a shortcut (one entry)
 *
 * 1. Add an entry to `BASE_SHORTCUTS` below: `id`, `keys`, `label`, `group`,
 *    `scope` and `handler`.
 * 2. With `handler: "dispatch"`, call `useShortcut(id, handler)` in the
 *    component that owns the action. The central dispatcher matches the key,
 *    checks the scope, ignores auto-repeat (unless `allowRepeat`) and calls
 *    the handler; return `false` from it to let the key through unhandled.
 *    Only the `global` and `workspace` scopes are dispatched centrally.
 * 3. With `handler: "external"`, the key is handled by its own component
 *    (name the file in `source`); the entry still documents it and takes
 *    part in the collision check.
 * 4. `npm test`: `shortcuts.test.ts` fails if two entries share a key in
 *    scopes that can be active at the same time, or if one binding is the
 *    start of another (a leader sequence and a plain chord).
 *
 * ## Key syntax
 *
 * `keys` lists alternative bindings. A binding is one chord, or a sequence of
 * chords separated by spaces (TUI-style leader: `"Ctrl+Space G"`).
 * A chord is `Modifier+...+Key`, with modifiers `Mod` (Cmd on macOS, Ctrl
 * elsewhere), `Ctrl`, `Meta`, `Alt` and `Shift`. Keys are `KeyboardEvent.key`
 * names (`Enter`, `Escape`, `ArrowDown`, `PageUp`, `Space`) or one character.
 * A single character with no Ctrl/Alt/Meta matches the character typed, case
 * included, so `"G"` is shift+g and `"?"` needs no `Shift+`. With Alt or
 * Ctrl/Meta, a letter or digit matches the physical key (`event.code`),
 * because Option changes `event.key` on macOS.
 *
 * ## Scopes
 *
 * A scope says where focus must be for the key to mean something. Scopes form
 * a tree (`SCOPES[scope].parent`); two entries can collide only when one
 * scope is the other or an ancestor of it. Sibling scopes are separated by
 * focus or by the layer in front (only the topmost dialog or menu takes
 * Escape), so `Escape` may mean different things in each. The TUI keeps bare
 * letters on its dashboard and a separate Ctrl+Space leader namespace in the
 * embedded view; here that is the `workspace` scope (bare letters, sequences)
 * versus `terminal`, which must stay free for the program running in it.
 */
import { useEffect, useRef } from "react";
import { DIFF_KEYS, type DiffCommand } from "./diffKeys";

export type ShortcutScope =
  | "global"
  | "workspace"
  | "sidebar-filter"
  | "composer"
  | "terminal"
  | "dialog"
  | "menu"
  | "tabs"
  | "changes"
  | "screenshot-viewer";

export const SCOPES: Record<ShortcutScope, { when: string; parent: ShortcutScope | null }> = {
  global: { when: "Anywhere, even in a text field or terminal", parent: null },
  workspace: { when: "Outside the terminal, text fields and dialogs", parent: "global" },
  "sidebar-filter": { when: "In the project filter", parent: "global" },
  composer: { when: "In the prompt composer", parent: "global" },
  terminal: { when: "In an agent or terminal session", parent: "global" },
  dialog: { when: "In the dialog or menu in front", parent: "global" },
  menu: { when: "In an open menu", parent: "global" },
  tabs: { when: "On a row of tabs or options", parent: "global" },
  changes: { when: "In Changes, outside its fields", parent: "dialog" },
  "screenshot-viewer": { when: "Viewing a screenshot", parent: "dialog" },
};

/** Display order of the dialog's sections. */
export const SHORTCUT_GROUPS = [
  "General",
  "Sidebar",
  "Prompt composer",
  "Terminal",
  "Dialogs and menus",
  "Changes",
  "Screenshots",
] as const;
export type ShortcutGroup = (typeof SHORTCUT_GROUPS)[number];

export interface ShortcutDef {
  id: string;
  /** Alternative bindings; see "Key syntax" above. */
  keys: readonly string[];
  label: string;
  group: ShortcutGroup;
  scope: ShortcutScope;
  /** `dispatch`: handled through `useShortcut`. `external`: handled by its
   *  own component, documented here. */
  handler: "dispatch" | "external";
  /** Where an external handler lives, for whoever migrates it. */
  source?: string;
  /** Fire on auto-repeat too (held keys). Off by default. */
  allowRepeat?: boolean;
}

const BASE_SHORTCUTS = [
  // General
  { id: "help.shortcuts", keys: ["?"], label: "Show keyboard shortcuts", group: "General", scope: "workspace", handler: "dispatch" },
  // Sidebar
  { id: "sidebar.filter.focus", keys: ["/"], label: "Filter projects and features", group: "Sidebar", scope: "workspace", handler: "dispatch" },
  { id: "sidebar.projects.toggle", keys: ["Alt+Shift+P"], label: "Show or hide the projects sidebar", group: "Sidebar", scope: "workspace", handler: "dispatch" },
  { id: "sidebar.agents.toggle", keys: ["Alt+Shift+A"], label: "Show or hide the agent sidebar", group: "Sidebar", scope: "workspace", handler: "dispatch" },
  { id: "sidebar.filter.open", keys: ["Enter"], label: "Open the first match", group: "Sidebar", scope: "sidebar-filter", handler: "external", source: "App.tsx (nav-filter input)" },
  { id: "sidebar.filter.clear", keys: ["Escape"], label: "Clear the filter and leave it", group: "Sidebar", scope: "sidebar-filter", handler: "external", source: "App.tsx (nav-filter input)" },
  // Prompt composer
  { id: "composer.send", keys: ["Mod+Enter"], label: "Send the prompt", group: "Prompt composer", scope: "composer", handler: "external", source: "PromptComposer.tsx" },
  { id: "composer.newline", keys: ["Enter"], label: "New line", group: "Prompt composer", scope: "composer", handler: "external", source: "PromptComposer.tsx (textarea default)" },
  // Terminal
  { id: "terminal.page-up", keys: ["Shift+PageUp"], label: "Scroll up a page", group: "Terminal", scope: "terminal", handler: "external", source: "terminalScroll.ts", allowRepeat: true },
  { id: "terminal.page-down", keys: ["Shift+PageDown"], label: "Scroll down a page", group: "Terminal", scope: "terminal", handler: "external", source: "terminalScroll.ts", allowRepeat: true },
  { id: "terminal.top", keys: ["Shift+Home"], label: "Scroll to the oldest output", group: "Terminal", scope: "terminal", handler: "external", source: "terminalScroll.ts" },
  { id: "terminal.latest", keys: ["Shift+End"], label: "Back to live output", group: "Terminal", scope: "terminal", handler: "external", source: "terminalScroll.ts" },
  { id: "terminal.reading", keys: ["PageUp", "PageDown", "Home", "End", "Escape"], label: "While reading earlier output: the same without Shift (Esc returns to live output)", group: "Terminal", scope: "terminal", handler: "external", source: "terminalScroll.ts" },
  // Dialogs and menus
  { id: "layer.close", keys: ["Escape"], label: "Close the dialog or menu in front", group: "Dialogs and menus", scope: "dialog", handler: "external", source: "ui.tsx (overlay layer stack)" },
  { id: "dialog.focus", keys: ["Tab", "Shift+Tab"], label: "Move focus (stays inside the dialog)", group: "Dialogs and menus", scope: "dialog", handler: "external", source: "ui.tsx (Modal)" },
  { id: "menu.move", keys: ["ArrowDown", "ArrowUp"], label: "Next or previous menu item (on the button: open the menu)", group: "Dialogs and menus", scope: "menu", handler: "external", source: "ui.tsx (Menu)" },
  { id: "menu.edge", keys: ["Home", "End"], label: "First or last menu item", group: "Dialogs and menus", scope: "menu", handler: "external", source: "ui.tsx (Menu)" },
  { id: "menu.choose", keys: ["Enter"], label: "Choose the menu item", group: "Dialogs and menus", scope: "menu", handler: "external", source: "ui.tsx (Menu)" },
  { id: "menu.close", keys: ["Tab"], label: "Close the menu", group: "Dialogs and menus", scope: "menu", handler: "external", source: "ui.tsx (Menu)" },
  { id: "tabs.move", keys: ["ArrowLeft", "ArrowRight"], label: "Previous or next tab or option", group: "Dialogs and menus", scope: "tabs", handler: "external", source: "ui.tsx (onTablistKeyDown, Segmented)" },
  { id: "tabs.edge", keys: ["Home", "End"], label: "First or last tab or option", group: "Dialogs and menus", scope: "tabs", handler: "external", source: "ui.tsx (onTablistKeyDown, Segmented)" },
  // Screenshots
  { id: "screenshots.move", keys: ["ArrowLeft", "ArrowRight"], label: "Previous or next screenshot", group: "Screenshots", scope: "screenshot-viewer", handler: "external", source: "ScreenshotViewer.tsx" },
] as const satisfies readonly ShortcutDef[];

/** Changes keys come from `DIFF_KEYS`, which its panel also dispatches on. */
const CHANGES_SHORTCUTS: ShortcutDef[] = DIFF_KEYS.map((entry) => ({
  id: `changes.${entry.command}`,
  keys: [entry.key],
  label: entry.help,
  group: "Changes",
  scope: "changes",
  handler: "external",
  source: "diffKeys.ts",
  allowRepeat: entry.command === "next-row" || entry.command === "prev-row",
}));

export type DispatchShortcutId = Extract<(typeof BASE_SHORTCUTS)[number], { handler: "dispatch" }>["id"];
export type ShortcutId = (typeof BASE_SHORTCUTS)[number]["id"] | `changes.${DiffCommand}`;

export const SHORTCUTS: readonly ShortcutDef[] = [...BASE_SHORTCUTS, ...CHANGES_SHORTCUTS];

export function shortcutById(id: ShortcutId): ShortcutDef {
  const def = SHORTCUTS.find((entry) => entry.id === id);
  if (!def) throw new Error(`Unknown shortcut ${id}`);
  return def;
}

// ---------------------------------------------------------------------------
// Parsing and display

export interface Chord {
  key: string;
  mod: boolean;
  ctrl: boolean;
  meta: boolean;
  alt: boolean;
  shift: boolean;
}

export function isMacPlatform(): boolean {
  return typeof navigator !== "undefined" && (navigator.platform ?? "").startsWith("Mac");
}

export function parseChord(text: string): Chord {
  const parts = text.split("+");
  // "Ctrl++" names the plus key.
  let key = parts.pop() ?? "";
  if (key === "" && parts.length > 0 && parts[parts.length - 1] === "") { parts.pop(); key = "+"; }
  const chord: Chord = { key, mod: false, ctrl: false, meta: false, alt: false, shift: false };
  for (const part of parts) {
    switch (part) {
      case "Mod": chord.mod = true; break;
      case "Ctrl": chord.ctrl = true; break;
      case "Meta": chord.meta = true; break;
      case "Alt": chord.alt = true; break;
      case "Shift": chord.shift = true; break;
      default: throw new Error(`Unknown modifier "${part}" in "${text}"`);
    }
  }
  if (!key) throw new Error(`Missing key in "${text}"`);
  return chord;
}

export function parseBinding(text: string): Chord[] {
  return text.trim().split(/\s+/).map(parseChord);
}

const isChar = (key: string) => key.length === 1;
const isAlnum = (key: string) => /^[a-z0-9]$/i.test(key);

/** The chord's identity on one platform, for comparing bindings. */
export function canonicalChord(chord: Chord, mac: boolean): string {
  const ctrl = chord.ctrl || (chord.mod && !mac);
  const meta = chord.meta || (chord.mod && mac);
  const plainChar = isChar(chord.key) && !ctrl && !meta && !chord.alt;
  const key = plainChar ? chord.key : isAlnum(chord.key) ? chord.key.toLowerCase() : chord.key;
  const mods = [ctrl && "ctrl", meta && "meta", chord.alt && "alt", !plainChar && chord.shift && "shift"].filter(Boolean);
  return [...mods, key].join("+");
}

/** Whether a key press is this chord on the given platform. */
export function chordMatches(event: KeyboardEvent, chord: Chord, mac = isMacPlatform()): boolean {
  const ctrl = chord.ctrl || (chord.mod && !mac);
  const meta = chord.meta || (chord.mod && mac);
  if (event.ctrlKey !== ctrl || event.metaKey !== meta || event.altKey !== chord.alt) return false;
  if (isChar(chord.key) && !ctrl && !meta && !chord.alt) return event.key === chord.key;
  if (event.shiftKey !== chord.shift) return false;
  if (isAlnum(chord.key)) {
    const code = /\d/.test(chord.key) ? `Digit${chord.key}` : `Key${chord.key.toUpperCase()}`;
    return event.code ? event.code === code : event.key.toLowerCase() === chord.key.toLowerCase();
  }
  return event.key === (chord.key === "Space" ? " " : chord.key);
}

const KEY_NAMES: Record<string, string> = {
  ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→",
  Escape: "Esc", PageUp: "Page Up", PageDown: "Page Down", " ": "Space",
};

/** Each chord as the keys to press, e.g. `["Ctrl", "Enter"]`. */
export function chordKeys(chord: Chord, mac = isMacPlatform()): string[] {
  const keys: string[] = [];
  if (chord.ctrl || (chord.mod && !mac)) keys.push("Ctrl");
  if (chord.meta || (chord.mod && mac)) keys.push(mac ? "Cmd" : "Meta");
  if (chord.alt) keys.push(mac ? "Option" : "Alt");
  if (chord.shift) keys.push("Shift");
  keys.push(KEY_NAMES[chord.key] ?? (isChar(chord.key) && (chord.alt || chord.ctrl || chord.meta || chord.mod) ? chord.key.toUpperCase() : chord.key));
  return keys;
}

/** A binding as text, e.g. "Alt+Shift+P" or "Ctrl+Space then G". */
export function formatBinding(binding: string, mac = isMacPlatform()): string {
  return parseBinding(binding).map((chord) => chordKeys(chord, mac).join("+")).join(" then ");
}

/** The first binding of a shortcut as text, for menus and tooltips. */
export function shortcutLabel(id: ShortcutId, mac = isMacPlatform()): string {
  return formatBinding(shortcutById(id).keys[0], mac);
}

/** The `aria-keyshortcuts` value for a shortcut (all its bindings). */
export function shortcutAria(id: ShortcutId, mac = isMacPlatform()): string {
  return shortcutById(id).keys.map((binding) => parseBinding(binding).map((chord) => {
    const names = chordKeys(chord, mac).map((key) => ({ Ctrl: "Control", Cmd: "Meta", Option: "Alt", "↑": "ArrowUp", "↓": "ArrowDown", "←": "ArrowLeft", "→": "ArrowRight", Esc: "Escape", "Page Up": "PageUp", "Page Down": "PageDown" } as Record<string, string>)[key] ?? key);
    return names.join("+");
  }).join(" ")).join(" ");
}

// ---------------------------------------------------------------------------
// Collisions

function scopeChain(scope: ShortcutScope): ShortcutScope[] {
  const chain: ShortcutScope[] = [];
  for (let s: ShortcutScope | null = scope; s; s = SCOPES[s].parent) chain.push(s);
  return chain;
}

/** Whether two scopes can be active for the same key press. */
export function scopesOverlap(a: ShortcutScope, b: ShortcutScope): boolean {
  return scopeChain(a).includes(b) || scopeChain(b).includes(a);
}

/** Every pair of entries whose bindings are equal, or one a prefix of the
 *  other, in overlapping scopes on either platform; plus duplicate ids. */
export function shortcutCollisions(defs: readonly ShortcutDef[] = SHORTCUTS): string[] {
  const problems: string[] = [];
  const ids = new Set<string>();
  for (const def of defs) {
    if (ids.has(def.id)) problems.push(`duplicate id ${def.id}`);
    ids.add(def.id);
  }
  for (const mac of [false, true]) {
    const bindings = defs.flatMap((def) => def.keys.map((text) => ({
      def, text, seq: parseBinding(text).map((chord) => canonicalChord(chord, mac)),
    })));
    for (let i = 0; i < bindings.length; i++) {
      for (let j = i + 1; j < bindings.length; j++) {
        const a = bindings[i], b = bindings[j];
        if (a.def === b.def || !scopesOverlap(a.def.scope, b.def.scope)) continue;
        const n = Math.min(a.seq.length, b.seq.length);
        if (a.seq.slice(0, n).join(" ") !== b.seq.slice(0, n).join(" ")) continue;
        const problem = `${a.def.id} (${a.text}) and ${b.def.id} (${b.text}) collide`;
        // Reported once: everywhere, or only on macOS when only there.
        if (!problems.includes(problem)) problems.push(mac ? `${problem} on macOS` : problem);
      }
    }
  }
  return problems;
}

// ---------------------------------------------------------------------------
// Dispatcher

/** Whether a key press belongs to a terminal, a form field or an open
 *  dialog, where workspace shortcuts must not fire. */
export function keyIsForOtherInput(event: KeyboardEvent): boolean {
  const target = event.target instanceof Element ? event.target : null;
  return !!target?.closest(".term-frame, input, textarea, select, [contenteditable]:not([contenteditable='false']), [role='textbox']")
    || hasVisibleDialog();
}

function hasVisibleDialog(): boolean {
  return Array.from(document.querySelectorAll("[role='dialog'], [data-confirm-callout]")).some((dialog) => {
    // Planning keeps its modal mounted under a hidden wrapper when minimized.
    if (dialog.closest("[hidden]")) return false;
    const visibility = window.getComputedStyle(dialog).visibility;
    if (visibility === "hidden" || visibility === "collapse") return false;
    for (let element: Element | null = dialog; element; element = element.parentElement) {
      if (window.getComputedStyle(element).display === "none") return false;
    }
    return true;
  });
}

function scopeIsActive(scope: ShortcutScope, event: KeyboardEvent): boolean {
  if (scope === "global") return true;
  if (scope === "workspace") return !keyIsForOtherInput(event);
  return false;
}

/** Return `false` to decline the key (it then passes through untouched). */
export type ShortcutHandler = (event: KeyboardEvent) => boolean | void;

const handlers = new Map<string, ShortcutHandler[]>();
let registry: readonly ShortcutDef[] = SHORTCUTS;

/** Swap the dispatcher's entries (tests of sequences, which the real registry
 *  does not use yet); `null` restores `SHORTCUTS`. */
export function setShortcutRegistryForTest(defs: readonly ShortcutDef[] | null) {
  registry = defs ?? SHORTCUTS;
  clearPending();
}
/** Chords typed so far of a multi-chord binding. */
let pending: KeyboardEvent[] = [];
let pendingTimer: ReturnType<typeof setTimeout> | null = null;
/** How long a leader sequence waits for its next key. */
export const SEQUENCE_TIMEOUT_MS = 1500;

function clearPending() {
  pending = [];
  if (pendingTimer) clearTimeout(pendingTimer);
  pendingTimer = null;
}

function dispatch(event: KeyboardEvent) {
  if (event.defaultPrevented || event.isComposing) return;
  const mac = isMacPlatform();
  const complete: { def: ShortcutDef; handler: ShortcutHandler }[] = [];
  let prefix = false;
  for (const def of registry) {
    if (def.handler !== "dispatch") continue;
    const stack = handlers.get(def.id);
    if (!stack?.length || !scopeIsActive(def.scope, event)) continue;
    if (event.repeat && !def.allowRepeat) continue;
    for (const binding of def.keys) {
      const seq = parseBinding(binding);
      if (seq.length <= pending.length) continue;
      if (!pending.every((earlier, index) => chordMatches(earlier, seq[index], mac))) continue;
      if (!chordMatches(event, seq[pending.length], mac)) continue;
      if (seq.length === pending.length + 1) complete.push({ def, handler: stack[stack.length - 1] });
      else prefix = true;
    }
  }
  for (const { handler } of complete) {
    if (handler(event) !== false) {
      event.preventDefault();
      clearPending();
      return;
    }
  }
  if (prefix) {
    event.preventDefault();
    pending = [...pending, event];
    if (pendingTimer) clearTimeout(pendingTimer);
    pendingTimer = setTimeout(clearPending, SEQUENCE_TIMEOUT_MS);
    return;
  }
  if (pending.length > 0) {
    // A key that continues no sequence starts over as a fresh press.
    clearPending();
    dispatch(event);
  }
}

function register(id: string, handler: ShortcutHandler): () => void {
  if (handlers.size === 0) window.addEventListener("keydown", dispatch);
  const stack = handlers.get(id) ?? [];
  stack.push(handler);
  handlers.set(id, stack);
  return () => {
    const current = handlers.get(id) ?? [];
    const index = current.indexOf(handler);
    if (index >= 0) current.splice(index, 1);
    if (current.length === 0) handlers.delete(id);
    if (handlers.size === 0) {
      window.removeEventListener("keydown", dispatch);
      clearPending();
    }
  };
}

/**
 * Binds a registry shortcut to a handler while the component is mounted.
 * The most recently mounted handler for an id wins. Pass `enabled: false` to
 * unbind without unmounting.
 */
export function useShortcut(id: DispatchShortcutId, handler: ShortcutHandler, enabled = true) {
  const latest = useRef(handler);
  latest.current = handler;
  useEffect(() => {
    if (!enabled) return;
    return register(id, (event) => latest.current(event));
  }, [id, enabled]);
}
