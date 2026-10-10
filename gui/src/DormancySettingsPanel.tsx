import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { asGuiError } from "./api";
import { Modal, Spinner } from "./ui";

interface SettingsView {
  settings: { idle_minutes: number; unattended_hours: number };
  revision: string;
}

export default function DormancySettingsPanel({ onClose, onSaved }: {
  onClose: () => void; onSaved: () => void;
}) {
  const [view, setView] = useState<SettingsView | null>(null);
  const [idle, setIdle] = useState("");
  const [unattended, setUnattended] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [discard, setDiscard] = useState<"close" | "reload" | null>(null);
  const inFlight = useRef(false);
  const dirty = !!view && (idle !== String(view.settings.idle_minutes)
    || unattended !== String(view.settings.unattended_hours));
  const valid = [idle, unattended].every((value) => /^\d+$/.test(value)
    && Number.isSafeInteger(Number(value)));

  async function load() {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true); setError(null);
    try {
      const fresh = await invoke<SettingsView>("dormancy_settings_load");
      setView(fresh);
      setIdle(String(fresh.settings.idle_minutes));
      setUnattended(String(fresh.settings.unattended_hours));
    } catch (err) { setError(asGuiError(err).message); }
    finally { inFlight.current = false; setBusy(false); }
  }
  useEffect(() => { void load(); }, []);

  function request(action: "close" | "reload") {
    if (inFlight.current) return;
    if (dirty) setDiscard(action);
    else if (action === "close") onClose();
    else void load();
  }
  async function save() {
    if (inFlight.current || !view || !valid) return;
    inFlight.current = true;
    setBusy(true); setError(null);
    try {
      await invoke<SettingsView>("dormancy_settings_save", {
        revision: view.revision,
        settings: { idle_minutes: Number(idle), unattended_hours: Number(unattended) },
      });
      onSaved();
    } catch (err) { setError(asGuiError(err).message); }
    finally { inFlight.current = false; setBusy(false); }
  }

  return <Modal label="Dormancy settings" title="Dormancy settings" onClose={() => request("close")}
    dismissable={!busy} subtitle="Global settings shared with the TUI. Nothing is stopped by saving settings."
    footer={<>
      <button className="btn btn-ghost" disabled={busy} onClick={() => request("close")}>Cancel</button>
      <button className="btn btn-secondary" disabled={busy} onClick={() => request("reload")}>Reload settings</button>
      <button className="btn btn-primary" disabled={busy || !view || !dirty || !valid} onClick={() => void save()}>Save settings</button>
    </>}>
    {busy && <p role="status"><Spinner /> Loading or saving settings…</p>}
    {error && <p className="callout callout-danger" role="alert">{error}</p>}
    {view && <div className="form-stack">
      <label>Idle minutes<input type="number" min="0" step="1" value={idle} disabled={busy}
        onChange={(event) => setIdle(event.target.value)} /></label>
      <label>Unattended hours<input type="number" min="0" step="1" value={unattended} disabled={busy}
        onChange={(event) => setUnattended(event.target.value)} /></label>
      <p className="muted">A feature must exceed both thresholds to appear. Set either value to 0 to turn detection off.
        Saved changes apply here immediately. Restart an already open TUI to use the new thresholds there.</p>
      {!valid && <p role="alert">Enter whole numbers of 0 or greater.</p>}
    </div>}
    {discard && <div role="alertdialog" aria-label="Discard unsaved settings" className="callout callout-warning">
      <p>Discard unsaved settings and {discard === "close" ? "close" : "reload"}?</p>
      <button className="btn btn-secondary" disabled={busy} onClick={() => {
        if (inFlight.current) return;
        setDiscard(null);
      }}>Keep editing</button>
      <button className="btn btn-warning" disabled={busy} onClick={() => {
        if (inFlight.current) return;
        const action = discard; setDiscard(null);
        if (action === "close") onClose(); else void load();
      }}>Discard changes</button>
    </div>}
  </Modal>;
}
