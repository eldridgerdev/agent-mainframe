import { useCallback, useEffect, useRef, useState } from "react";
import { asGuiError, FeatureTarget } from "./api";
import { Spinner } from "./ui";
import {
  ExplanationView, explanationPrepare, explanationStart, explanationPoll, explanationCancel,
} from "./supervisedEditExplanationsApi";

/** Owns only explanation state. Retiring a generation also retires IPC results
 * already in flight; their matching owner is cancelled rather than adopted. */
export function useEditExplanation(target: FeatureTarget, edit?: { id: string; revision: string; answered: boolean; unavailable: string | null }) {
  const [view, setView] = useState<ExplanationView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [approval, setApproval] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  const owned = useRef<ExplanationView | null>(null);
  const pending = useRef<Promise<unknown> | null>(null);
  const retirements = useRef(new Set<Promise<unknown>>());
  const unacknowledged = useRef(new Set<string>());
  const cancellationErrors = useRef(new Map<string, unknown>());
  const cancelOwner = useCallback((owner: string) => {
    unacknowledged.current.add(owner);
    const request = explanationCancel(owner).then((result) => {
      unacknowledged.current.delete(owner);
      cancellationErrors.current.delete(owner);
      return result;
    }).catch((failure) => {
      // A concurrent successful acknowledgement already retired this owner.
      if (unacknowledged.current.has(owner)) cancellationErrors.current.set(owner, failure);
      throw failure;
    });
    retirements.current.add(request);
    void request.finally(() => retirements.current.delete(request)).catch(() => {});
    return request;
  }, []);
  const adopt = useCallback(async (result: ExplanationView, epoch: number) => {
    if (generation.current !== epoch) {
      if ("owner" in result) await cancelOwner(result.owner);
      return;
    }
    owned.current = result;
    if ("owner" in result && (result.state === "precall" || result.state === "running")) {
      unacknowledged.current.add(result.owner);
    }
    setView(result);
  }, [cancelOwner]);
  const cancel = useCallback(async () => {
    generation.current += 1;
    const previous = owned.current;
    owned.current = null;
    setView(null); setApproval(null); setError(null); setNotice(null); setBusy(false);
    if (previous && "owner" in previous) unacknowledged.current.add(previous.owner);
    try {
      // A failed acknowledgement keeps the owner retryable on the next Close.
      await Promise.all([...unacknowledged.current].map(cancelOwner));
      await pending.current;
      if (cancellationErrors.current.size) throw cancellationErrors.current.values().next().value;
      // An in-flight preparation may have acquired an owner while retiring.
      await Promise.all([...unacknowledged.current].map(cancelOwner));
      await Promise.all([...retirements.current]);
    } catch (failure) {
      setError(asGuiError(failure).message);
      throw failure;
    }
  }, [cancelOwner]);
  const identity = JSON.stringify([target.project_id, target.feature_id, edit?.id, edit?.revision, edit?.answered, edit?.unavailable]);
  useEffect(() => {
    void cancel().catch((failure) => setError(asGuiError(failure).message));
    return () => { void cancel().catch(() => {}); };
  }, [identity, cancel]);

  async function perform(action: () => Promise<ExplanationView>, canApprove = false) {
    if (pending.current) return;
    const epoch = generation.current;
    setBusy(true); setError(null); setApproval(null); setNotice(null);
    const request = action().then((result) => adopt(result, epoch)).catch((failure) => {
      if (generation.current !== epoch) return;
      const problem = asGuiError(failure);
      if (canApprove && problem.kind === "needs_approval") setApproval(problem.message);
      else setError(problem.message);
    }).finally(() => { if (pending.current === request) pending.current = null; if (generation.current === epoch) setBusy(false); });
    pending.current = request;
    await request;
  }
  const owner = view && "owner" in view ? view.owner : null;
  useEffect(() => {
    if (view?.state !== "running") return;
    const epoch = generation.current;
    let running = false;
    const timer = window.setInterval(() => {
      if (running) return;
      running = true;
      void explanationPoll(view.owner, view.request).then((result) => adopt(result, epoch)).catch((failure) => {
        if (generation.current === epoch) {
          setError(asGuiError(failure).message);
          setView(null);
        }
      }).finally(() => { running = false; });
    }, 300);
    return () => window.clearInterval(timer);
  }, [view, adopt]);
  async function cancelCall() {
    const wasNotice = view?.state === "precall";
    const completion = cancel();
    const epoch = generation.current;
    await completion;
    if (generation.current === epoch && owned.current === null && pending.current === null) {
      setNotice(wasNotice ? "Explanation cancelled before starting an AI call."
        : "Explanation call cancelled. The edit is still waiting.");
    }
  }
  return { view, error, approval, notice, busy, cancel, cancelCall,
    prepare: () => edit && perform(() => explanationPrepare(target, edit.id, edit.revision)),
    start: (approved = false) => owner && perform(() => explanationStart(owner, approved), true),
  };
}

type State = ReturnType<typeof useEditExplanation>;
export function EditExplanation({ explanation: state, disabled }: { explanation: State; disabled: boolean }) {
  return <section aria-label="Edit explanation" className="supervised-explanation">
    <button className="btn btn-secondary btn-sm" disabled={disabled || state.busy || state.view?.state === "running" || state.view?.state === "precall"}
      onClick={() => void state.prepare()}>{state.busy && <Spinner />}Explain this edit</button>
    {state.error && <p role="alert">{state.error}</p>}
    {state.notice && <p role="status">{state.notice}</p>}
    {state.view?.state === "precall" && <div role="alertdialog" aria-label="Explanation AI call" className="callout callout-warning supervised-confirm">
      <p><strong>Call Claude to explain this edit?</strong> This starts a separate AI call with no file or repository tools. It does not answer the waiting agent.</p>
      <p>Model: {state.view.model ?? "Claude default"} · Reasoning: {state.view.reasoning ?? "Claude default"}</p>
      <details><summary>Exact prompt</summary><pre className="supervised-snippet">{state.view.prompt}</pre></details>
      {state.approval && <p role="alert">{state.approval}</p>}
      <div className="supervised-actions">
        <button className="btn btn-ghost" disabled={state.busy} onClick={() => void state.cancelCall().catch(() => {})}>Cancel explanation</button>
        <button className="btn btn-primary" disabled={state.busy} onClick={() => void state.start(Boolean(state.approval))}>{state.approval ? "Continue anyway" : "Continue explanation"}</button>
      </div>
    </div>}
    {state.view?.state === "running" && <p role="status"><Spinner /> Explaining this edit… <button className="btn btn-ghost btn-sm" onClick={() => void state.cancelCall().catch(() => {})}>Cancel explanation</button></p>}
    {state.view?.state === "failed" && <p role="alert">{state.view.message}</p>}
    {(state.view?.state === "complete" || state.view?.state === "saved_note") && <div className="callout callout-accent">
      <strong>{state.view.state === "saved_note" ? "Saved review note" : "AI explanation"}</strong>
      <pre className="supervised-snippet">{state.view.text || "The saved note is empty."}</pre>
    </div>}
  </section>;
}
