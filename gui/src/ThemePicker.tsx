import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { Field, Icon, Modal } from "./ui";
import "./appearance.css";
import { DEFAULT_SIZING, INTERFACE_SCALES, setSizingPreference, useGuiSizing } from "./sizing";
import { applyTheme, DEFAULT_THEME_PREFERENCE, GuiTheme, receiveThemePreference, resolveTheme, setThemePreference, ThemeCatalog, useThemePreference } from "./themes";

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
        receiveThemePreference(event.newValue || DEFAULT_THEME_PREFERENCE);
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);
  return { choice, catalog, ...sizing };
}

/** Catalog themes split by light/dark so a long list can be scanned; custom
 * files get their own group because they are the user's. */
function themeGroups(themes: GuiTheme[]) {
  const custom = themes.filter((theme) => theme.id.startsWith("custom:"));
  const catalog = themes.filter((theme) => !theme.id.startsWith("custom:"));
  return [
    { label: "Light themes", themes: catalog.filter((theme) => theme.mode === "light") },
    { label: "Dark themes", themes: catalog.filter((theme) => theme.mode === "dark") },
    { label: "Themes that follow the system", themes: catalog.filter((theme) => theme.mode === "system") },
    { label: "Your custom themes", themes: custom },
  ].filter((group) => group.themes.length > 0);
}

export const TERMINAL_FONT_MIN = 11;
export const TERMINAL_FONT_MAX = 28;

/** A number field with -/+ buttons. The field keeps what is typed until it
 * is a whole size in range, so clearing it to type a new value does not reset it. */
function SizeStepper({ value, onChange }: { value: number; onChange: (size: number) => void }) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  const commit = (raw: string) => {
    setDraft(raw);
    const size = Number(raw);
    if (Number.isInteger(size) && size >= TERMINAL_FONT_MIN && size <= TERMINAL_FONT_MAX) onChange(size);
  };
  return <div className="field">
    <span className="field-label" aria-hidden="true">Terminal text size</span>
    <div className="size-stepper">
      <button type="button" className="btn btn-secondary btn-icon" aria-label="Smaller terminal text"
        disabled={value <= TERMINAL_FONT_MIN} onClick={() => onChange(value - 1)}><Icon name="minimize" size={14} /></button>
      <input type="number" aria-label="Terminal text size" inputMode="numeric" min={TERMINAL_FONT_MIN} max={TERMINAL_FONT_MAX} step={1}
        value={draft} onChange={(event) => commit(event.target.value)} onBlur={() => setDraft(String(value))} />
      <span className="muted">px{value === 13 ? " (default)" : ""}</span>
      <button type="button" className="btn btn-secondary btn-icon" aria-label="Larger terminal text"
        disabled={value >= TERMINAL_FONT_MAX} onClick={() => onChange(value + 1)}><Icon name="plus" size={14} /></button>
    </div>
    <span className="field-hint">Agent and terminal text, from {TERMINAL_FONT_MIN} to {TERMINAL_FONT_MAX}px, before interface scaling.</span>
  </div>;
}

export default function ThemePicker({ onClose, appearance }: { onClose: () => void; appearance: ReturnType<typeof useGuiTheme> }) {
  const { choice, catalog, sizing, error } = appearance;
  const missing = catalog.data && !resolveTheme(catalog.data, choice);
  return <Modal label="Appearance" title="Appearance" onClose={onClose}>
    <div className="form-stack">
    <Field label="Theme" hint={<>Match the AMF terminal app (the default) uses the theme set in AMF's terminal (TUI) app and picks up changes there within a few seconds. Follow system uses your desktop's light or dark setting. Your choice is saved on this device and never changes the terminal app.</>}>
      <select aria-label="Theme" value={choice} onChange={(event) => setThemePreference(event.target.value)}>
        <optgroup label="Automatic">
          <option value="follow-tui">Match the AMF terminal app (default)</option>
          <option value="system">Follow system</option>
        </optgroup>
        <optgroup label="Built-in">
          <option value="light">Light</option>
          <option value="dark">Dark</option>
        </optgroup>
        {themeGroups(catalog.data?.themes ?? []).map((group) => <optgroup key={group.label} label={group.label}>
          {group.themes.map((theme) => <option key={theme.id} value={theme.id}>{theme.name}</option>)}
        </optgroup>)}
        {missing && <option value={choice}>Unavailable: {choice}</option>}
      </select>
    </Field>
    {missing && <p role="status">The selected theme is unavailable. Using the system palette until it is restored or another theme is selected.</p>}
    {catalog.isError && <p role="alert">Could not load themes. Built-in choices remain available. <button className="btn btn-secondary btn-sm" onClick={() => void catalog.refetch()}>Retry</button></p>}
    <Field label="Interface size" hint="Enlarges text, buttons and terminal together. Try 125% or 150% on a laptop. Changes apply immediately and are remembered on this device.">
      <select aria-label="Interface size" value={sizing.interfaceScale} onChange={(event) => setSizingPreference({ ...sizing, interfaceScale: Number(event.target.value) })}>
        {INTERFACE_SCALES.map((scale) => <option key={scale} value={scale}>{scale}%{scale === 100 ? " (default)" : ""}</option>)}
      </select>
    </Field>
    {error && <p role="alert">{error}</p>}
    <SizeStepper value={sizing.terminalFontSize} onChange={(terminalFontSize) => setSizingPreference({ ...sizing, terminalFontSize })} />
    <div className="row">
      <button type="button" className="btn btn-secondary btn-sm" onClick={() => setSizingPreference(DEFAULT_SIZING)}>Reset sizes</button>
    </div>
    {catalog.data && <><p className="small">Custom JSON themes: <span className="mono">{catalog.data.directory}</span>. Files reload automatically.</p>
      {catalog.data.errors.map((error) => <p role="alert" key={error}>{error}</p>)}
      {!!catalog.data.warnings?.length && <div role="group" aria-label="Custom theme contrast">
        <p className="small">Some custom theme colours are hard to read. They are used as written:</p>
        <ul className="small">{catalog.data.warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul>
      </div>}</>}
    </div>
  </Modal>;
}
