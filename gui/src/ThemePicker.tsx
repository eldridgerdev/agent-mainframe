import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { Modal } from "./ui";
import { DEFAULT_SIZING, INTERFACE_SCALES, setSizingPreference, useGuiSizing } from "./sizing";
import { applyTheme, receiveThemePreference, resolveTheme, setThemePreference, ThemeCatalog, useThemePreference } from "./themes";

export function useGuiTheme() {
  const choice = useThemePreference();
  const sizing = useGuiSizing();
  const catalog = useQuery({
    queryKey: ["theme-catalog"],
    queryFn: async () => {
      const result = await invoke<ThemeCatalog>("theme_catalog");
      if (!Array.isArray(result?.themes)) throw new Error("Theme catalog unavailable");
      return result;
    },
    refetchInterval: 3000,
    refetchIntervalInBackground: true,
  });
  useEffect(() => { applyTheme(resolveTheme(catalog.data, choice)); }, [catalog.data, choice]);
  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key === "amf.gui.theme" || event.key === null) {
        // Another GUI window already saved this; adopt it without writing back.
        receiveThemePreference(event.newValue || "follow-tui");
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);
  return { choice, catalog, ...sizing };
}

export default function ThemePicker({ onClose, appearance }: { onClose: () => void; appearance: ReturnType<typeof useGuiTheme> }) {
  const { choice, catalog, sizing, error } = appearance;
  const missing = catalog.data && !resolveTheme(catalog.data, choice);
  return <Modal label="Appearance" title="Appearance" onClose={onClose}>
    <div className="form-stack">
    <label className="field">Interface size
      <select aria-label="Interface size" value={sizing.interfaceScale} onChange={(event) => setSizingPreference({ ...sizing, interfaceScale: Number(event.target.value) })}>
        {INTERFACE_SCALES.map((scale) => <option key={scale} value={scale}>{scale}%{scale === 100 ? " (default)" : ""}</option>)}
      </select>
    </label>
    <p className="muted small">Enlarges text, buttons and terminal together. Try 125% or 150% on a laptop. Changes apply immediately and are remembered on this device.</p>
    {error && <p role="alert">{error}</p>}
    <label className="field">Terminal text size
      <select aria-label="Terminal text size" value={sizing.terminalFontSize} onChange={(event) => setSizingPreference({ ...sizing, terminalFontSize: Number(event.target.value) })}>
        {Array.from({ length: 18 }, (_, i) => i + 11).map((size) => <option key={size} value={size}>{size}px{size === 13 ? " (default)" : ""}</option>)}
      </select>
    </label>
    <p className="muted small">Adjusts agent and terminal text separately, before interface scaling.</p>
    <div className="row">
      <button type="button" className="btn btn-secondary btn-sm" onClick={() => setSizingPreference(DEFAULT_SIZING)}>Reset sizes</button>
    </div>
    <label className="field">Theme
      <select aria-label="Theme" value={choice} onChange={(event) => setThemePreference(event.target.value)}>
        <option value="follow-tui">Follow TUI theme</option>
        <option value="system">Follow system</option>
        <option value="light">Light</option>
        <option value="dark">Dark</option>
        {catalog.data?.themes.map((theme) => <option key={theme.id} value={theme.id}>{theme.name}</option>)}
        {missing && <option value={choice}>Unavailable: {choice}</option>}
      </select>
    </label>
    <p className="muted small">GUI choices are remembered independently. Follow TUI theme picks up changes made in the TUI within a few seconds.</p>
    {missing && <p role="status">The selected theme is unavailable. Using the system palette until it is restored or another theme is selected.</p>}
    {catalog.isError && <p role="alert">Could not load themes. Built-in choices remain available. <button onClick={() => void catalog.refetch()}>Retry</button></p>}
    {catalog.data && <><p className="small">Custom JSON themes: <span className="mono">{catalog.data.directory}</span>. Files reload automatically.</p>
      {catalog.data.errors.map((error) => <p role="alert" key={error}>{error}</p>)}
      {!!catalog.data.warnings?.length && <div role="group" aria-label="Custom theme contrast">
        <p className="small">Some custom theme colours are hard to read. They are used as written:</p>
        <ul className="small">{catalog.data.warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul>
      </div>}</>}
    </div>
  </Modal>;
}
