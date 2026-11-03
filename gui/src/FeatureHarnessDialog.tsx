import { useEffect, useRef, useState } from "react";
import { asGuiError, type AgentSlug, type FeatureTarget } from "./api";
import { featureHarnessApply, featureHarnessLoad, type HarnessChangeView } from "./harnessChangeApi";
import { Field, Modal, Spinner } from "./ui";

export default function FeatureHarnessDialog({ target, hasDrafts, sending, onClose, onResync, onChanged }: {
  target: FeatureTarget;
  hasDrafts: boolean;
  sending: boolean;
  onClose: () => void;
  onResync: () => void;
  onChanged: () => void;
}) {
  const [view, setView] = useState<HarnessChangeView | null>(null);
  const [agent, setAgent] = useState<AgentSlug | "">("");
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const mounted = useRef(true);
  async function load() {
    if (inFlight.current) return;
    inFlight.current = true; setBusy(true); setError(null); setView(null); setConfirmed(false);
    try {
      const fresh = await featureHarnessLoad(target);
      if (mounted.current) { setView(fresh); setAgent(fresh.current_agent); }
    } catch (err) { if (mounted.current) setError(asGuiError(err).message); }
    finally { inFlight.current = false; if (mounted.current) setBusy(false); }
  }
  useEffect(() => { mounted.current = true; void load(); return () => { mounted.current = false; }; }, []);
  const choice = view?.choices.find((choice) => choice.agent === agent);
  const canApply = !!view && !!choice && !choice.unavailable && agent !== view.current_agent
    && confirmed && !hasDrafts && !sending && !busy;
  async function apply() {
    if (!canApply || !view || !agent || inFlight.current) return;
    inFlight.current = true; setBusy(true); setError(null);
    try {
      await featureHarnessApply(target, view.revision, agent);
      onChanged(); onClose();
    } catch (err) {
      if (mounted.current) { setError(asGuiError(err).message); setView(null); setConfirmed(false); }
    } finally {
      // A failure can follow shutdown, hook changes or a partially completed
      // restart. Always refresh the workspace and terminal attachments.
      onResync(); inFlight.current = false; if (mounted.current) setBusy(false);
    }
  }
  return <Modal label="Change agent" title="Change agent" subtitle={view?.feature_name}
    dismissable={!busy} onClose={() => { if (!inFlight.current) onClose(); }}
    footer={<><button className="btn btn-secondary" disabled={busy} onClick={onClose}>Cancel</button>
      <button className="btn btn-secondary" disabled={busy} onClick={() => void load()}>Reload</button>
      <button className="btn btn-primary" disabled={!canApply} onClick={() => void apply()}>Change agent</button></>}>
    {busy && <Spinner />}
    {error && <p className="error-message" role="alert">{error}</p>}
    {(hasDrafts || sending) && <p role="alert">{sending ? "Wait for the current prompt send to finish." : "This feature has unsent agent drafts. Cancel and send or explicitly clear them before changing the agent."}</p>}
    {view && <>
      <Field label="New agent"><select value={agent} disabled={busy} onChange={(event) => {
        setAgent(event.target.value as AgentSlug); setConfirmed(false);
      }}>
        {view.choices.map((choice) => <option key={choice.agent} value={choice.agent} disabled={!!choice.unavailable}>
          {choice.name}{choice.unavailable ? ` — ${choice.unavailable}` : ""}
        </option>)}
      </select></Field>
      {!view.choices.some((choice) => !choice.unavailable && choice.agent !== view.current_agent)
        && <p>No other installed agent is allowed for this feature and mode.</p>}
      <p>{view.agent_rows ? `All ${view.agent_rows} agent session rows will switch` : "An agent session row will be added"} to the selected agent. Their past conversations can no longer be resumed. Terminal and custom session definitions stay the same.</p>
      <p>{view.running ? "The running feature and all its sessions will stop. Previously runnable sessions restart; individually stopped sessions remain stopped." : "The feature will remain stopped. This change does not launch any sessions."}</p>
      <label className="checkbox-row"><input type="checkbox" checked={confirmed} disabled={busy || agent === view.current_agent}
        onChange={(event) => setConfirmed(event.target.checked)} /> I understand and confirm this agent change</label>
    </>}
  </Modal>;
}
