import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { asGuiError } from "./api";
import { Field, Modal, Spinner } from "./ui";

interface SettingsView {
  settings: { window_limit: string; warning_percent: string; critical_percent: string };
  revision: string;
}

export default function ContextSettingsPanel({ onClose, onSaved }: {
  onClose: () => void; onSaved: () => void;
}) {
  const [view, setView] = useState<SettingsView | null>(null);
  const [windowLimit, setWindowLimit] = useState("");
  const [warning, setWarning] = useState("");
  const [critical, setCritical] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [discard, setDiscard] = useState<"close" | "reload" | null>(null);
  const inFlight = useRef(false);
  const dirty = !!view && (windowLimit !== view.settings.window_limit
    || warning !== view.settings.warning_percent || critical !== view.settings.critical_percent);
  const whole = (value: string) => /^\d+$/.test(value);
  const validWindow = windowLimit === "" || (whole(windowLimit) && BigInt(windowLimit) > 0n && BigInt(windowLimit) <= 18446744073709551615n);
  const valid = validWindow && [warning, critical].every(value => whole(value) && Number(value) >= 1 && Number(value) <= 100)
    && Number(critical) > Number(warning);

  async function load() {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true); setError(null);
    try {
      const fresh = await invoke<SettingsView>("context_settings_load");
      setView(fresh);
      setWindowLimit(fresh.settings.window_limit);
      setWarning(fresh.settings.warning_percent);
      setCritical(fresh.settings.critical_percent);
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
      await invoke<SettingsView>("context_settings_save", {
        revision: view.revision,
        settings: { window_limit: windowLimit, warning_percent: warning, critical_percent: critical },
      });
      onSaved();
    } catch (err) { setError(asGuiError(err).message); }
    finally { inFlight.current = false; setBusy(false); }
  }

  return <Modal label="Context settings" title="Context settings" onClose={() => request("close")}
    dismissable={!busy} subtitle="Global context display settings shared with the TUI. Existing sessions keep running."
    footer={<>
      <button className="btn btn-ghost" disabled={busy} onClick={() => request("close")}>Cancel</button>
      <button className="btn btn-secondary" disabled={busy} onClick={() => request("reload")}>Reload settings</button>
      <button className="btn btn-primary" disabled={busy || !view || !dirty || !valid} onClick={() => void save()}>Save settings</button>
    </>}>
    {busy && <p role="status"><Spinner /> Loading or saving settings…</p>}
    {error && <p className="callout callout-danger" role="alert">{error}</p>}
    {view && <div className="form-stack">
      <Field label="Context window (tokens)" hint="Leave blank to restore harness defaults. Used when telemetry does not report a limit.">
        <input type="text" inputMode="numeric" aria-label="Context window (tokens)" value={windowLimit} disabled={busy}
          onChange={(event) => setWindowLimit(event.target.value)} />
      </Field>
      <Field label="Warning percent">
        <input type="number" min="1" max="99" step="1" value={warning} disabled={busy}
          onChange={(event) => setWarning(event.target.value)} />
      </Field>
      <Field label="Critical percent">
        <input type="number" min="2" max="100" step="1" value={critical} disabled={busy}
          onChange={(event) => setCritical(event.target.value)} />
      </Field>
      <p className="muted">Threshold changes update the desktop indicators immediately. A changed window limit marks estimated samples stale
        until the next context update. External settings edits refresh automatically. Restart an already open TUI after saving: until then
        it keeps its old values and writes them back when it next saves a setting.</p>
      {!valid && <p role="alert">Use a positive whole token limit or leave it blank. Percentages must be 1–100, with critical greater than warning.</p>}
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
