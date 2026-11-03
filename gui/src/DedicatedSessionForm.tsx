import { useEffect, useRef, useState } from "react";
import { AgentSlug, asGuiError } from "./api";
import {
  DedicatedCatalog, PrDestinationPrepared, PrDestinationPreview, PrDestinationWorkflow,
  dedicatedCatalog, destinationPrepare, destinationPreview,
} from "./prDestinationApi";
import { Field } from "./ui";

const NAME_DEBOUNCE_MS = 250;

/** Create (or reuse) the dedicated "PR Triage" agent session of the opened
 * feature. A read-only preview comes first; "Create (stopped)" repeats every
 * check on the backend and creates a session that is NOT running: no process,
 * no prompt, nothing posted to GitHub. The caller gets the created target back
 * and continues into its existing fix draft or batch preview. */
export default function DedicatedSessionForm({ workflow, expected, onCreated, onClose }: {
  workflow: PrDestinationWorkflow;
  /** The PR the surrounding panel is showing; a different live PR is a conflict. */
  expected?: { pr_number: number; head_sha: string } | null;
  onCreated: (result: PrDestinationPrepared) => void;
  onClose: () => void;
}) {
  const [catalog, setCatalog] = useState<DedicatedCatalog | null>(null);
  const [agent, setAgent] = useState<AgentSlug | "">("");
  const [name, setName] = useState("");
  const [committedName, setCommittedName] = useState("");
  const [preview, setPreview] = useState<{ key: string; value: PrDestinationPreview } | null>(null);
  const [approved, setApproved] = useState(false);
  const [needsApproval, setNeedsApproval] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<PrDestinationPrepared | null>(null);
  const [reload, setReload] = useState(0);
  const submitting = useRef(false);
  const previewSeq = useRef(0);

  useEffect(() => {
    let live = true;
    setCatalog(null);
    dedicatedCatalog(workflow).then((next) => {
      if (!live) return;
      if (expected && (expected.pr_number !== next.identity.pr_number || expected.head_sha !== next.identity.head_sha)) {
        setError("The PR changed since this panel loaded. Refresh the panel, then reopen this form.");
        return;
      }
      setCatalog(next);
      setAgent((current) => current && next.agents.some((a) => a.agent === current && !a.unavailable) ? current
        : (next.agents.find((a) => a.agent === next.preferred && !a.unavailable) ?? next.agents.find((a) => !a.unavailable))?.agent ?? "");
    }).catch((err) => { if (live) setError(asGuiError(err).message); });
    return () => { live = false; };
  }, [workflow.kind, workflow.workflow_id, reload]);

  useEffect(() => {
    const timer = window.setTimeout(() => setCommittedName(name), NAME_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [name]);

  const inputKey = `${agent}|${name.trim()}`;
  const committedKey = `${agent}|${committedName.trim()}`;
  useEffect(() => {
    if (!catalog || !agent || done) return;
    const seq = ++previewSeq.current;
    setPreview(null);
    setApproved(false);
    setNeedsApproval(null);
    destinationPreview(workflow, catalog.identity, { kind: "dedicated_session", agent, name: committedName.trim() || null })
      .then((value) => { if (previewSeq.current === seq) { setPreview({ key: committedKey, value }); setError(null); } })
      .catch((err) => { if (previewSeq.current === seq) setError(asGuiError(err).message); });
  }, [catalog, committedKey, reload]);

  const current = preview && preview.key === inputKey ? preview.value : null;
  const gate = current?.resource_warning ?? needsApproval;
  const reuses = !!current?.reuses_existing;
  const canSubmit = !!current && !busy && !done && (!gate || approved);

  const submit = async () => {
    // A second click in the same tick must not start a second creation.
    if (submitting.current || !catalog || !current || !canSubmit) return;
    submitting.current = true;
    setBusy(true);
    try {
      const result = await destinationPrepare(workflow, catalog.identity, { kind: "dedicated_session", agent: current.agent, name: committedName.trim() || null }, current.revision, approved);
      if (result.status === "in_progress") {
        setError(result.message);
        submitting.current = false;
        return;
      }
      setDone(result);
      setError(null);
      onCreated(result);
    } catch (err) {
      const failure = asGuiError(err);
      if (failure.kind === "needs_approval") setNeedsApproval(failure.message);
      else { setError(failure.message); setPreview(null); }
      submitting.current = false;
    } finally {
      setBusy(false);
    }
  };

  const unavailable = catalog?.agents.filter((a) => a.unavailable) ?? [];
  return (
    <section className="review-editor dedicated-session-form" aria-label="Dedicated triage session">
      <h3>Dedicated triage session</h3>
      <p>
        A separate agent session in <strong>{catalog?.feature_name ?? "this feature"}</strong> for PR fixes.
        The agent is created <strong>stopped</strong>: nothing runs and no prompt is sent. To start it, open its
        session tab and choose Start. Nothing is posted to GitHub.
      </p>
      {error && <p className="callout callout-danger" role="alert">{error} <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => { setError(null); setReload((n) => n + 1); }}>Refresh</button></p>}
      {catalog && <>
        <Field label="Agent">
          <select value={agent} disabled={busy || !!done} onChange={(event) => setAgent(event.target.value as AgentSlug)}>
            {catalog.agents.map((a) => <option key={a.agent} value={a.agent} disabled={!!a.unavailable}>{a.label}{a.unavailable ? ` (unavailable)` : ""}</option>)}
          </select>
        </Field>
        {unavailable.length > 0 && <p className="small muted">{unavailable.map((a) => `${a.label}: ${a.unavailable}`).join(" · ")}</p>}
        <Field label="Session name (optional)" hint={`Blank uses “${catalog.default_name}”. The name identifies the session.`}>
          <input type="text" value={name} maxLength={60} placeholder={catalog.default_name} disabled={busy || !!done} onChange={(event) => setName(event.target.value)} />
        </Field>
      </>}
      {!current && !error && catalog && <p className="muted small" role="status">Checking what would be created…</p>}
      {current && <div aria-label="What will happen">
        {reuses && <p className="callout callout-accent" role="status">A session named “{current.label}” already exists in {current.feature_name}. It is reused; nothing new is created.</p>}
        <ul>{current.consequences.map((line) => <li key={line}>{line}</li>)}</ul>
      </div>}
      {gate && <label className="row small">
        <input type="checkbox" checked={approved} disabled={busy || !!done} onChange={(event) => setApproved(event.target.checked)} />
        <span>I approve despite the resource warning: {gate}</span>
      </label>}
      {done && <p className="callout callout-accent" role="status">{done.message}</p>}
      <div className="row">
        {!done && <button type="button" className="btn btn-primary" disabled={!canSubmit} onClick={() => void submit()}>{busy ? "Working…" : reuses ? "Use existing session" : "Create (stopped)"}</button>}
        <button type="button" className="btn btn-secondary" disabled={busy} onClick={onClose}>{done ? "Close" : "Cancel"}</button>
      </div>
    </section>
  );
}
