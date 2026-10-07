import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { asGuiError, type SessionTarget } from "./api";
import { Modal, Spinner } from "./ui";

interface Preview { revision: string; prompt: string; label: string }
interface Result { target: SessionTarget; draft_prompt: string }

export default function FreshContextDialog({ target, onCreated, onClose }: {
  target: SessionTarget;
  onCreated: (target: SessionTarget, draft: string) => void;
  onClose: () => void;
}) {
  const client = useQueryClient();
  const preview = useQuery({ queryKey: ["fresh-context", target],
    queryFn: () => invoke<Preview>("fresh_context_preview", { target }), staleTime: 0,
    refetchOnWindowFocus: false, gcTime: 0, retry: false });
  const [draft, setDraft] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [approval, setApproval] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [discard, setDiscard] = useState(false);
  const busy = useRef(false);
  const text = draft ?? preview.data?.prompt ?? "";
  const dirty = draft !== null && draft !== preview.data?.prompt;
  const close = () => { if (busy.current) return; if (dirty) setDiscard(true); else onClose(); };
  const start = async (approved = false) => {
    if (busy.current || !preview.data || preview.isFetching || !text.trim()) return;
    busy.current = true;
    setPending(true); setError(null);
    try {
      const result = await invoke<Result>("fresh_context_start", { target,
        request: { revision: preview.data.revision, prompt: text, approved } });
      void client.invalidateQueries({ queryKey: ["workspace"] });
      onCreated(result.target, result.draft_prompt);
      onClose();
    } catch (err) {
      const e = asGuiError(err);
      if (e.kind === "needs_approval") setApproval(e.message);
      else { setApproval(null); setError(e.message); }
    } finally { busy.current = false; setPending(false); }
  };
  return <Modal label="Fresh context" title="Fresh context" size="lg" onClose={close}
    footer={<>
      <button className="btn btn-secondary" disabled={pending} onClick={close}>Cancel</button>
      <button className="btn btn-primary" disabled={pending || preview.isFetching || !preview.data || !!preview.error || !text.trim() || !!approval}
        onClick={() => void start()}>{pending ? "Starting…" : "Start fresh context"}</button>
    </>}>
    <div data-unsaved-changes={dirty}>
      <p>Start a new agent session with the feature’s configured harness. Your current session stays open. The continuation opens as an unsent composer draft for you to review.</p>
      {preview.isPending && <p><Spinner /> Preparing context…</p>}
      {preview.error && <p role="alert">{asGuiError(preview.error).message}</p>}
      {preview.data && <label className="field">Continuation prompt
        <textarea aria-label="Continuation prompt" className="plan-textarea mono" rows={10} value={text} disabled={pending}
          onChange={(e) => { setDraft(e.target.value); setApproval(null); }} />
      </label>}
      {error && <p role="alert">{error}</p>}
      {(error || preview.error) && <button className="btn btn-secondary" disabled={pending || preview.isFetching}
        onClick={() => { setApproval(null); setError(null); void preview.refetch(); }}>Reload context (keep draft)</button>}
      {approval && <div role="alertdialog" aria-label="Resource warning">
        <p>{approval}</p>
        <button className="btn btn-primary" disabled={pending} onClick={() => void start(true)}>Start anyway</button>
        <button className="btn btn-secondary" disabled={pending} onClick={() => setApproval(null)}>Keep editing</button>
      </div>}
      {discard && <div role="alertdialog" aria-label="Discard continuation draft">
        <p>Discard your edited continuation prompt?</p>
        <button className="btn btn-danger" onClick={onClose}>Discard draft</button>
        <button className="btn btn-secondary" onClick={() => setDiscard(false)}>Keep editing</button>
      </div>}
    </div>
  </Modal>;
}
