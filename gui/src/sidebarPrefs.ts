import { useCallback, useSyncExternalStore } from "react";
import { useShortcut } from "./shortcuts";

export { keyIsForOtherInput } from "./shortcuts";

// Per-viewer layout preferences for the GUI's sidebars. They belong to this
// window's user, not to the workspace: nothing here reaches the shared
// database, so the TUI's own sidebars are unaffected. Storage can be missing
// or refuse writes (private windows, cleared site data), so every access is
// guarded and the in-memory value still works for the session.
//
// Projects and agent panels have independent preferences. The projects
// sidebar's tool links fold into a "Tools" group that starts collapsed, so
// the project tree is what a first look lands on.

export type SidebarPref = "sessionSidebar" | "projectsSidebar" | "workspaceTools";

const DEFAULT_COLLAPSED: Record<SidebarPref, boolean> = {
  sessionSidebar: false,
  projectsSidebar: false,
  workspaceTools: true,
};

const STORAGE_PREFIX = "amf.gui.collapsed.";
const memory = new Map<SidebarPref, boolean>();
const listeners = new Set<() => void>();

function read(pref: SidebarPref): boolean {
  const cached = memory.get(pref);
  if (cached !== undefined) return cached;
  let stored = DEFAULT_COLLAPSED[pref];
  try {
    const value = window.localStorage.getItem(STORAGE_PREFIX + pref);
    if (value !== null) stored = value === "1";
  } catch {
    // Unavailable storage: start from the default.
  }
  memory.set(pref, stored);
  return stored;
}

export function setSidebarCollapsed(pref: SidebarPref, collapsed: boolean) {
  memory.set(pref, collapsed);
  try {
    window.localStorage.setItem(STORAGE_PREFIX + pref, collapsed ? "1" : "0");
  } catch {
    // Kept in memory for this window only.
  }
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/** Whether `pref`'s sidebar is collapsed, remembered across restarts. */
export function useSidebarCollapsed(pref: SidebarPref): [boolean, (collapsed: boolean) => void] {
  const collapsed = useSyncExternalStore(subscribe, () => read(pref), () => DEFAULT_COLLAPSED[pref]);
  const set = useCallback((next: boolean) => setSidebarCollapsed(pref, next), [pref]);
  return [collapsed, set];
}

/** Forget cached values (tests reset storage between cases). */
export function resetSidebarPrefsForTest() {
  memory.clear();
}

/** Shortcuts apply only to workspace chrome, never to terminal or form input
 *  (see `shortcuts.ts`, which owns the bindings). */
export function useSidebarShortcut(pref: "sessionSidebar" | "projectsSidebar", toggle: () => void) {
  useShortcut(pref === "projectsSidebar" ? "sidebar.projects.toggle" : "sidebar.agents.toggle", () => { toggle(); });
}
