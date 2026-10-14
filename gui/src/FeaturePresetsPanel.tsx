import { useEffect, useRef, useState } from "react";
import { asGuiError, FeaturePreset, featurePresetsLoad, featurePresetsSave, PresetRow, PresetsView } from "./api";
import { Field, Modal, Spinner } from "./ui";

const emptyPreset = (): FeaturePreset => ({ name: "", branch_prefix: null, mode: "vibe", agent: "claude",
  review: false, plan_mode: false, quick_plan: false, enable_chrome: false, remote_control: false });

export default function FeaturePresetsPanel({ onClose }: { onClose: () => void }) {
  const [view, setView] = useState<PresetsView | null>(null);
  const [rows, setRows] = useState<PresetRow[]>([]);
  const [selected, setSelected] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [discard, setDiscard] = useState<"close" | "reload" | null>(null);
  const inFlight = useRef(false);
  const dirty = !!view && JSON.stringify(rows) !== JSON.stringify(view.rows);
  const names = rows.map((row) => row.preset.name.trim());
  const namesValid = names.every(Boolean) && new Set(names).size === names.length;
  const unsupported = (item: PresetRow) => item.preset.agent === "codex" && item.preset.mode === "vibeless";
  // Stored rows left untouched only warn; new or edited rows must not add the pair.
  const storedUnchanged = (item: PresetRow) => item.original_index !== null
    && JSON.stringify(view?.rows[item.original_index]?.preset) === JSON.stringify(item.preset);
  const blocking = rows.filter((item) => unsupported(item) && !storedUnchanged(item));
  const valid = namesValid && !blocking.length;
  const row = rows[selected];

  async function load() {
    if (inFlight.current) return;
    inFlight.current = true; setBusy(true); setError(null);
    try {
      const fresh = await featurePresetsLoad();
      setView(fresh); setRows(fresh.rows); setSelected(0);
    } catch (err) { setError(asGuiError(err).message); }
    finally { inFlight.current = false; setBusy(false); }
  }
  useEffect(() => { void load(); }, []);
  function request(action: "close" | "reload") {
    if (inFlight.current) return;
    if (dirty) setDiscard(action);
    else if (action === "close") onClose(); else void load();
  }
  function edit(patch: Partial<FeaturePreset>) {
    setRows((previous) => previous.map((item, index) => index === selected
      ? { ...item, preset: { ...item.preset, ...patch } } : item));
  }
  async function save() {
    if (inFlight.current || !view || !valid) return;
    inFlight.current = true; setBusy(true); setError(null);
    try {
      const fresh = await featurePresetsSave(view.revision, rows);
      setView(fresh); setRows(fresh.rows);
    } catch (err) { setError(asGuiError(err).message); }
    finally { inFlight.current = false; setBusy(false); }
  }

  return <Modal size="lg" label="Feature presets" title="Feature presets" onClose={() => request("close")}
    dismissable={!busy} subtitle="Global templates shared with the TUI. Saving changes only the templates for future features."
    footer={<>
      <button className="btn btn-ghost" disabled={busy} onClick={() => request("close")}>Cancel</button>
      <button className="btn btn-secondary" disabled={busy} onClick={() => request("reload")}>Reload presets</button>
      <button className="btn btn-primary" disabled={busy || !view || !dirty || !valid} onClick={() => void save()}>Save presets</button>
    </>}>
    {busy && <p role="status"><Spinner /> Loading or saving presets…</p>}
    {error && <p className="callout callout-danger" role="alert">{error}</p>}
    {view && <div className="form-stack">
      <div className="library-toolbar">
        <Field label="Stored preset"><select value={rows.length ? selected : ""} disabled={busy || !rows.length}
          onChange={(event) => setSelected(Number(event.target.value))}>
          {!rows.length && <option value="">No presets yet</option>}
          {rows.map((item, index) => <option key={index} value={index}>{item.preset.name || "Unnamed preset"}</option>)}
        </select></Field>
        <button className="btn btn-secondary" disabled={busy} onClick={() => {
          setRows([...rows, { original_index: null, preset: emptyPreset() }]); setSelected(rows.length);
        }}>Add preset</button>
        <button className="btn btn-danger" disabled={busy || !row} onClick={() => {
          setRows(rows.filter((_, index) => index !== selected)); setSelected(Math.max(0, selected - 1));
        }}>Delete preset</button>
      </div>
      {row && <>
        <Field label="Preset name"><input value={row.preset.name} disabled={busy} onChange={(event) => edit({ name: event.target.value })} /></Field>
        <Field label="Branch prefix"><input value={row.preset.branch_prefix ?? ""} disabled={busy}
          placeholder="feat" onChange={(event) => edit({ branch_prefix: event.target.value || null })} /></Field>
        <div className="form-row">
          <Field label="Harness"><select value={row.preset.agent} disabled={busy} onChange={(event) => edit({ agent: event.target.value as FeaturePreset["agent"] })}>
            <option value="claude">Claude Code</option><option value="codex">Codex</option><option value="opencode">OpenCode</option><option value="pi">Pi</option>
          </select></Field>
          <Field label="Mode"><select value={row.preset.mode} disabled={busy} onChange={(event) => edit({ mode: event.target.value as FeaturePreset["mode"] })}>
            <option value="vibeless">Vibeless</option><option value="vibe">Vibe</option><option value="supervibe">SuperVibe</option>
          </select></Field>
        </div>
        {unsupported(row) && <p className="callout callout-warning" role="alert">Codex does not support Vibeless. Choose Vibe or SuperVibe, or another harness.{
          storedUnchanged(row) ? " Features cannot be created from this stored preset until it is fixed." : " Save is blocked until this is fixed."}</p>}
        <Field label="Planning"><select value={!row.preset.plan_mode ? "none" : row.preset.quick_plan ? "quick" : "full"} disabled={busy}
          onChange={(event) => edit({ plan_mode: event.target.value !== "none", quick_plan: event.target.value === "quick" })}>
          <option value="none">Start directly</option><option value="quick">Quick Plan</option><option value="full">Full Plan</option>
        </select></Field>
        {([ ["review", "Review"], ["enable_chrome", "Chrome"], ["remote_control", "Remote Control"] ] as const).map(([key, label]) =>
          <label className="switch-row" key={key}><input className="switch" type="checkbox" checked={row.preset[key]} disabled={busy} onChange={(event) => edit({ [key]: event.target.checked })} /><span className="switch-label">{label}</span></label>)}
        <p className="muted">Project presets with the same name override these templates. Creation offers presets whose harness is allowed and installed.
          Remote Control templates can be used in the TUI; desktop creation requires explicitly clearing that option.</p>
      </>}
      {!namesValid && <p role="alert">Use a nonempty, unique name for every preset.</p>}
      {blocking.some((item) => item !== row) && <p role="alert">Fix the Codex + Vibeless combination in: {
        blocking.filter((item) => item !== row).map((item) => item.preset.name || "Unnamed preset").join(", ")}.</p>}
    </div>}
    {discard && <div role="alertdialog" aria-label="Discard unsaved presets" className="callout callout-warning">
      <p>Discard unsaved presets and {discard === "close" ? "close" : "reload"}?</p>
      <button className="btn btn-secondary" disabled={busy} onClick={() => setDiscard(null)}>Keep editing</button>
      <button className="btn btn-warning" disabled={busy} onClick={() => {
        if (inFlight.current) return;
        const action = discard; setDiscard(null);
        if (action === "close") onClose(); else void load();
      }}>Discard changes</button>
    </div>}
  </Modal>;
}
