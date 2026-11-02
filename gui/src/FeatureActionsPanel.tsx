import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { asGuiError, FeatureTarget } from "./api";
import { Modal } from "./ui";

export interface FeatureActionsView {
  name: string;
  nickname: string | null;
  ready: boolean;
  summary: string | null;
  summary_generating: boolean;
  sessions: { id: string; label: string }[];
  summary_outcome: string | null;
}
interface SummaryNotice {
  prompt_id: string; title: string; harness: string; harness_name: string; template: string; revision: string;
}

// Rename a feature (its nickname) or a session, mark ready, and generate the
// AI summary: the TUI's `r`, `y` and `Z`. Every write echoes the value this
// panel last showed (`seen`), so a change made elsewhere is refused by the
// backend instead of overwritten; the panel never adopts a newer value by
// itself while a draft is open, it offers a Reload.
export default function FeatureActionsPanel({ target, focusSessionId, onClose, onChanged }: {
  target: FeatureTarget; focusSessionId?: string | null; onClose: () => void; onChanged: () => void;
}) {
  const query = useQuery({ queryKey: ["feature-actions", target.feature_id],
    queryFn: () => invoke<FeatureActionsView>("feature_actions_load", { target }),
    retry: false, staleTime: 0, refetchInterval: 2000 });
  const [seen, setSeen] = useState<FeatureActionsView | null>(null);
  const [nickname, setNickname] = useState("");
  const [labels, setLabels] = useState<Record<string, string>>({});
  const [notice, setNotice] = useState<SummaryNotice | null>(null);
  const [outcome, setOutcome] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const live = query.data;

  function adopt(view: FeatureActionsView) {
    setSeen(view); setNickname(view.nickname ?? "");
    setLabels(Object.fromEntries(view.sessions.map(s => [s.id, s.label])));
  }
  useEffect(() => { if (live && !seen) adopt(live); }, [live, seen]);
  useEffect(() => {
    if (live?.summary_outcome) { setOutcome(live.summary_outcome); onChanged(); }
  }, [live?.summary_outcome]);

  const changedElsewhere = !!(seen && live && (live.nickname !== seen.nickname || live.ready !== seen.ready
    || JSON.stringify(live.sessions) !== JSON.stringify(seen.sessions)));

  async function run(command: string, args: Record<string, unknown>, after?: (view: FeatureActionsView) => void) {
    if (inFlight.current) return;
    inFlight.current = true; setBusy(true); setError("");
    try {
      const view = await invoke<FeatureActionsView>(command, args);
      adopt(view); after?.(view); onChanged(); void query.refetch();
    } catch (cause) { setError(asGuiError(cause).message); }
    finally { inFlight.current = false; setBusy(false); }
  }
  async function showNotice() {
    if (inFlight.current) return;
    inFlight.current = true; setBusy(true); setError(""); setOutcome("");
    try { setNotice(await invoke<SummaryNotice>("feature_summary_notice", { target })); }
    catch (cause) { setError(asGuiError(cause).message); }
    finally { inFlight.current = false; setBusy(false); }
  }
  async function confirmSummary() {
    if (!notice || inFlight.current) return;
    inFlight.current = true; setBusy(true); setError("");
    try {
      const ack = { prompt_id: notice.prompt_id, harness: notice.harness, revision: notice.revision };
      await invoke<FeatureActionsView>("feature_summary_generate", { target, ack });
      setNotice(null); void query.refetch();
    } catch (cause) { setError(asGuiError(cause).message); setNotice(null); }
    finally { inFlight.current = false; setBusy(false); }
  }

  const close = () => { if (!inFlight.current) onClose(); };
  const ready = seen?.ready ?? false;
  return <Modal label="Feature actions" title="Rename, ready and summary" onClose={close}
    subtitle={live?.name ?? seen?.name}
    footer={<button className="btn btn-secondary" disabled={busy} onClick={close}>Close</button>}>
    {(error || query.error) && <p role="alert" className="callout callout-danger">{error || asGuiError(query.error).message}</p>}
    {changedElsewhere && <p role="status" className="callout callout-warning">
      This feature changed elsewhere. <button className="btn btn-secondary" disabled={busy}
        onClick={() => live && adopt(live)}>Reload</button></p>}
    {!seen && !query.error && <p role="status">Loading…</p>}
    {seen && <div className="feature-actions">
      <section aria-label="Nickname">
        <label>Nickname <input value={nickname} disabled={busy} placeholder="Shown instead of the feature name; empty clears it"
          onChange={e => setNickname(e.target.value)} /></label>
        <button className="btn btn-primary" disabled={busy || nickname === (seen.nickname ?? "")}
          onClick={() => void run("feature_rename", { target, expected: seen.nickname, nickname })}>Save nickname</button>
        <p className="muted">Changes only the display name; the branch, worktree and tmux session keep their names.</p>
      </section>
      <section aria-label="Sessions">
        {seen.sessions.map(session => <div key={session.id} className="row">
          <label>{focusSessionId === session.id ? "Session name (selected)" : "Session name"}
            <input aria-label={`Name of session ${session.label}`} value={labels[session.id] ?? ""} disabled={busy}
              autoFocus={focusSessionId === session.id}
              onChange={e => setLabels(current => ({ ...current, [session.id]: e.target.value }))} /></label>
          <button className="btn btn-primary" aria-label={`Rename session ${session.label}`}
            disabled={busy || (labels[session.id] ?? "") === session.label}
            onClick={() => void run("session_rename", {
              target: { ...target, session_id: session.id }, expected: session.label, label: labels[session.id] ?? "" })}>Rename</button>
        </div>)}
      </section>
      <section aria-label="Ready">
        <p>{ready ? "Marked ready." : "Not marked ready."}</p>
        <button className="btn btn-secondary" disabled={busy}
          onClick={() => void run("feature_set_ready", { target, expected: ready })}>{ready ? "Mark not ready" : "Mark ready"}</button>
      </section>
      <section aria-label="Summary">
        <p>{live?.summary_generating ? "Generating summary…" : (live?.summary ?? seen.summary) ? `Summary: ${live?.summary ?? seen.summary}` : "No summary yet."}</p>
        {outcome && <p role="status">{outcome}</p>}
        {!notice && <button className="btn btn-secondary" disabled={busy || !!live?.summary_generating}
          onClick={() => void showNotice()}>Generate summary…</button>}
        {notice && <div role="group" aria-label="Headless AI call" className="callout callout-warning">
          <p><strong>This runs a headless AI call</strong> ({notice.title}, <code>{notice.prompt_id}</code>) on {notice.harness_name}.
            It reads the recent output of the feature's agent window and may use model quota.</p>
          <pre aria-label="Prompt template">{notice.template}</pre>
          <div className="row">
            <button className="btn btn-primary" disabled={busy} onClick={() => void confirmSummary()}>Run with {notice.harness_name}</button>
            <button className="btn btn-secondary" disabled={busy} onClick={() => setNotice(null)}>Cancel</button>
          </div>
        </div>}
      </section>
    </div>}
  </Modal>;
}
