import { useEffect, useState, useSyncExternalStore } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";

export const INTERFACE_SCALES = [80, 90, 100, 110, 125, 150, 175, 200];
export const DEFAULT_SIZING = { interfaceScale: 100, terminalFontSize: 13 };
export type GuiSizing = typeof DEFAULT_SIZING;
const key = "amf.gui.sizing";
const listeners = new Set<() => void>();
let preference: GuiSizing | undefined;

function parseSizing(raw: string | null): GuiSizing {
  try {
    const value = JSON.parse(raw ?? "null");
    return {
      interfaceScale: INTERFACE_SCALES.includes(value?.interfaceScale) ? value.interfaceScale : 100,
      terminalFontSize: Number.isInteger(value?.terminalFontSize) && value.terminalFontSize >= 11 && value.terminalFontSize <= 28
        ? value.terminalFontSize : 13,
    };
  } catch { return DEFAULT_SIZING; }
}

export function readSizingPreference(): GuiSizing {
  if (preference) return preference;
  try { preference = parseSizing(window.localStorage.getItem(key)); }
  catch { preference = DEFAULT_SIZING; }
  return preference;
}

function receiveSizing(value: GuiSizing) {
  preference = value;
  listeners.forEach((listener) => listener());
}

export function setSizingPreference(value: GuiSizing) {
  const next = parseSizing(JSON.stringify(value));
  try { window.localStorage.setItem(key, JSON.stringify(next)); }
  catch { /* Keep sizing usable when storage is unavailable. */ }
  receiveSizing(next);
}

export function useSizingPreference() {
  return useSyncExternalStore((listener) => {
    listeners.add(listener);
    return () => { listeners.delete(listener); };
  }, readSizingPreference, () => DEFAULT_SIZING);
}

// Serialize native requests so fast changes cannot leave an older zoom applied.
let zoomQueue = Promise.resolve();
export function useGuiSizing() {
  const sizing = useSizingPreference();
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    // Vite's browser preview has no native webview. Desktop zoom adjusts the
    // layout viewport as well as text, including media queries and xterm.
    if (!("__TAURI_INTERNALS__" in window)) return;
    zoomQueue = zoomQueue.catch(() => {}).then(async () => {
      if (cancelled) return;
      await getCurrentWebview().setZoom(sizing.interfaceScale / 100);
    });
    void zoomQueue.then(() => {
      if (!cancelled) setError(null);
    }, () => {
      if (!cancelled) setError("Could not apply interface size. Choose another size to retry.");
    });
    return () => { cancelled = true; };
  }, [sizing.interfaceScale]);
  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key === key || event.key === null) receiveSizing(parseSizing(event.newValue));
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);
  return { sizing, error };
}

export function resetSizingForTest() { preference = undefined; }
