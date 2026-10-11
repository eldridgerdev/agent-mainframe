import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AgentSlug, asGuiError } from "./api";
import { Modal, Spinner } from "./ui";

interface SettingsView {
  settings: { allowed_agents: AgentSlug[] };
  revision: string;
}

const agents: [AgentSlug, string][] = [["claude", "Claude Code"], ["codex", "Codex"], ["opencode", "OpenCode"], ["pi", "Pi"]];

export default function AgentAvailabilityPanel({ onClose, onSaved }: {
  onClose: () => void; onSaved: () => void;
}) {
  const [view, setView] = useState<SettingsView | null>(null);
  const [allowed, setAllowed] = useState<AgentSlug[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [discard, setDiscard] = useState<"close" | "reload" | null>(null);
  const inFlight = useRef(false);
  const dirty = !!view && agents.some(([agent]) => allowed.includes(agent) !== view.settings.allowed_agents.includes(agent));
  const valid = allowed.length > 0;

  async function load() {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true); setError(null);
    try {
      const fresh = await invoke<SettingsView>("agent_availability_load");
      setView(fresh);
      setAllowed(fresh.settings.allowed_agents);
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
      await invoke<SettingsView>("agent_availability_save", {
        revision: view.revision,
        settings: { allowed_agents: agents.map(([agent]) => agent).filter(agent => allowed.includes(agent)) },
      });
      onSaved();
    } catch (err) { setError(asGuiError(err).message); }
    finally { inFlight.current = false; setBusy(false); }
  }

  return <Modal label="Agent availability" title="Agent availability" onClose={() => request("close")}
    dismissable={!busy} subtitle="Global choices shared with the TUI. Existing sessions keep running."
    footer={<>
      <button className="btn btn-ghost" disabled={busy} onClick={() => request("close")}>Cancel</button>
      <button className="btn btn-secondary" disabled={busy} onClick={() => request("reload")}>Reload settings</button>
      <button className="btn btn-primary" disabled={busy || !view || !dirty || !valid} onClick={() => void save()}>Save settings</button>
    </>}>
    {busy && <p role="status"><Spinner /> Loading or saving settings…</p>}
    {error && <p className="callout callout-danger" role="alert">{error}</p>}
    {view && <div className="form-stack">
      {agents.map(([agent, label]) => <label key={agent} className={`option ${allowed.includes(agent) ? "option-selected" : ""}`}>
        <input type="checkbox" checked={allowed.includes(agent)} disabled={busy}
          onChange={(event) => setAllowed(current => event.target.checked ? [...current, agent] : current.filter(value => value !== agent))} /> {label}
      </label>)}
      <p className="muted">Choose the globally allowed agents for new features and sessions. Project allowed-agent settings take precedence.
        Installed harness availability also applies. This does not install or sign in to an agent.</p>
      {!valid && <p role="alert">Select at least one agent.</p>}
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
