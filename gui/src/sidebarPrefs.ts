import { useCallback, useEffect, useSyncExternalStore } from "react";

// Per-viewer layout preferences for the GUI's sidebars. They belong to this
// window's user, not to the workspace: nothing here reaches the shared
// database, so the TUI's own sidebars are unaffected. Storage can be missing
// or refuse writes (private windows, cleared site data), so every access is
// guarded and the in-memory value still works for the session.
//
// Projects and agent panels have independent preferences.

export type SidebarPref = "sessionSidebar" | "projectsSidebar";

const STORAGE_PREFIX = "amf.gui.collapsed.";
const memory = new Map<SidebarPref, boolean>();
const listeners = new Set<() => void>();

function read(pref: SidebarPref): boolean {
  const cached = memory.get(pref);
  if (cached !== undefined) return cached;
  let stored = false;
  try {
    stored = window.localStorage.getItem(STORAGE_PREFIX + pref) === "1";
  } catch {
    // Unavailable storage: start expanded.
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
  const collapsed = useSyncExternalStore(subscribe, () => read(pref), () => false);
  const set = useCallback((next: boolean) => setSidebarCollapsed(pref, next), [pref]);
  return [collapsed, set];
}

/** Forget cached values (tests reset storage between cases). */
export function resetSidebarPrefsForTest() {
  memory.clear();
}

/** Shortcuts apply only to workspace chrome, never to terminal or form input. */
export function useSidebarShortcut(pref: SidebarPref, toggle: () => void) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const key = pref === "projectsSidebar" ? "p" : "a";
      if (event.defaultPrevented || event.repeat || !event.altKey || !event.shiftKey
          || event.ctrlKey || event.metaKey
          || (event.code ? event.code !== `Key${key.toUpperCase()}` : event.key.toLowerCase() !== key)) return;
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest(".term-frame, input, textarea, select, [contenteditable]:not([contenteditable='false']), [role='textbox']")
          || document.querySelector("[role='dialog'], [role='alertdialog']")) return;
      event.preventDefault();
      toggle();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [pref, toggle]);
}
