import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { asGuiError, SessionTarget } from "./api";
import { Modal } from "./ui";

export interface AttentionRow {
  id: string; modified_ms: number | null;
  project_id: string | null; feature_id: string | null;
  project_name: string; feature_name: string;
  kind: string; reason: string; message: string;
  age: string | null; age_secs: number | null; stale: boolean;
  session: SessionTarget | null; jump_note: string | null;
  dismissable: boolean; dismiss_note: string | null;
}
export interface NeedsAttentionView {
  rows: AttentionRow[]; hidden_stopped: number; hidden_resumed: number; limits: string[];
}
export const NEEDS_ATTENTION_KEY = ["needs-attention"];
export const loadNeedsAttention = () => invoke<NeedsAttentionView>("needs_attention_load");

/** The GUI's counterpart of the TUI's `i` list. It lists only requests that
 * hooks left on disk; the TUI's Question/Completed/Waiting labels live in the
 * TUI process and are stated as a limit, never guessed. */
export default function NeedsAttentionPanel({ onJump, onClose }: {
  onJump: (target: SessionTarget) => void | Promise<void>; onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const query = useQuery({ queryKey: NEEDS_ATTENTION_KEY, queryFn: loadNeedsAttention,
    retry: false, staleTime: 0, refetchInterval: 3000 });
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const [error, setError] = useState("");
  async function run(action: () => Promise<void>) {
    if (inFlight.current) return;
    inFlight.current = true; setBusy(true); setError("");
    try { await action(); } catch (cause) { setError(asGuiError(cause).message); }
    finally { inFlight.current = false; setBusy(false); }
  }
  const view = query.data;
  return <Modal label="Needs attention" title="Needs attention" size="lg" onClose={() => { if (!inFlight.current) onClose(); }}
    subtitle="Requests that agents left waiting on disk. Jumping only opens the session; it never starts or answers anything."
    footer={<button className="btn btn-secondary" disabled={busy} onClick={onClose}>Close</button>}>
    {(error || query.error) && <p role="alert">{error || asGuiError(query.error).message}{query.error && view && " Showing the previous load."}</p>}
    {view?.limits.map(limit => <p key={limit} className="muted small" role="note">{limit}</p>)}
    {view && view.rows.length === 0 && <p>Nothing listed needs attention.</p>}
    {!view && !query.error && <p role="status">Loading…</p>}
    {view && view.rows.length > 0 && <ol aria-label="Needs attention" className="attention-list">
      {view.rows.map(row => <li key={row.id} className="attention-row">
        <div className="attention-head">
          <strong>{row.project_name} / {row.feature_name}</strong>
          <span className="attention-reason">{row.reason}</span>
          {row.age && <span className="muted small" title={row.stale ? "Older than your waiting-stale setting" : undefined}>{row.age}{row.stale ? " (stale)" : ""}</span>}
        </div>
        {row.message && <p className="attention-message">{row.message}</p>}
        {row.jump_note && <p className="muted small">{row.jump_note}</p>}
        {row.dismiss_note && <p className="muted small">{row.dismiss_note}</p>}
        <div className="row">
          <button className="btn btn-secondary" disabled={busy || !row.session}
            aria-label={`Open session for ${row.feature_name}`}
            onClick={() => row.session && void run(async () => { await onJump(row.session!); onClose(); })}>Open session</button>
          <button className="btn btn-ghost" disabled={busy || !row.dismissable}
            aria-label={`Dismiss ${row.feature_name}: ${row.reason}`}
            onClick={() => void run(async () => {
              const next = await invoke<NeedsAttentionView>("needs_attention_dismiss", { id: row.id, modifiedMs: row.modified_ms });
              queryClient.setQueryData(NEEDS_ATTENTION_KEY, next);
            })}>Dismiss</button>
        </div>
      </li>)}
    </ol>}
  </Modal>;
}
