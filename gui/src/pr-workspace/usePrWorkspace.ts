import { useCallback, useEffect, useRef, useState, type MutableRefObject } from "react";
import { asGuiError, type FeatureTarget, type GuiError } from "../api";
import { prTriageAct, prTriageBegin, type PrTriageView } from "../prTriageApi";
import { workspaceBegin, workspaceClose, workspaceSelectPr, workspaceSwitchAi, workspaceSwitchThreads } from "../prWorkspaceApi";

export type Tab = "files" | "threads" | "ai";

/** What the lifecycle needs from the AI review: load it when the AI tab opens, and forget it when the PR changes. */
export type AiLifecycle = { load: (workflowId: string) => Promise<void>; reset: () => void };

/** The PR workspace's lifecycle: opening and releasing the backend workspace, switching tabs, following the
 * selected PR and closing. The AI review state lives in useAiReview; `ai` lets this hook load and reset it. */
export function usePrWorkspace({ target, onClose, onOpenPrReview, ai }: { target: FeatureTarget; onClose: () => void; onOpenPrReview?: (projectId: string) => void; ai: MutableRefObject<AiLifecycle | null> }) {
  const [workflowId, setWorkflowId] = useState<string | null>(null);
  const [triage, setTriage] = useState<PrTriageView | null>(null);
  const [filesDirty, setFilesDirty] = useState(false);
  const [tab, setTab] = useState<Tab>("threads");
  const handoffPending = useRef(false);
  const handoffGeneration = useRef(0);
  const [busy, setBusy] = useState(true);
  const [triageBusy, setTriageBusy] = useState(true);
  // The last refusal, kept whole so a GitHub problem can name its fix.
  const [failure, setFailure] = useState<GuiError | null>(null);
  const error = failure?.message ?? null;
  const setError = useCallback((message: string | null) => setFailure(message === null ? null : { kind: "internal", message }), []);
  // Bumped by Retry when the workspace itself could not open.
  const [attempt, setAttempt] = useState(0);
  const tabRef = useRef(tab);
  tabRef.current = tab;
  const prIdentity = useRef<string | null>(null);
  const triageClose = useRef<(() => void) | null>(null);
  const mountGeneration = useRef(0);

  useEffect(() => {
    const generation = ++mountGeneration.current;
    let live = true;
    let id: string | null = null;
    let initialized = false;
    const release = () => {
      if (!live && initialized && id && mountGeneration.current === generation) {
        void workspaceClose(id).catch(() => undefined);
        id = null;
      }
    };
    void (async () => {
      try {
        id = await workspaceBegin(target);
        if (!live) {
          if (mountGeneration.current === generation) { initialized = true; release(); }
          return;
        }
        const view = await prTriageBegin(target);
        initialized = true;
        if (!live) { release(); return; }
        setWorkflowId(id); setTriage(view);
      } catch (e) {
        initialized = true;
        if (id && mountGeneration.current === generation) {
          void workspaceClose(id).catch(() => undefined);
          id = null;
        }
        if (live) setFailure(asGuiError(e));
      } finally { if (live) setBusy(false); }
    })();
    return () => {
      live = false;
      handoffGeneration.current++;
      queueMicrotask(release);
    };
  }, [target.project_id, target.feature_id, attempt]);

  const switchTab = useCallback(async (next: Tab) => {
    if (!workflowId || busy || triageBusy) return;
    setBusy(true); setError(null);
    try {
      if (next === "ai") {
        const number = triage?.review?.number;
        if (number == null) throw new Error("Open a pull request in Threads first.");
        await workspaceSelectPr(workflowId, number);
        await workspaceSwitchAi(workflowId, number);
        await ai.current?.load(workflowId);
      } else {
        await workspaceSwitchThreads(workflowId);
      }
      setTab(next);
    } catch (e) { setFailure(asGuiError(e)); }
    finally { setBusy(false); }
  }, [workflowId, busy, triageBusy, triage]);

  const closeWorkspace = useCallback(async (triageAlreadyClosed = false, thenReview = false) => {
    if (!workflowId) { onClose(); return; }
    if (busy) return;
    setBusy(true);
    try {
      if (tab !== "threads") await workspaceSwitchThreads(workflowId);
      if (triage && !triageAlreadyClosed) await prTriageAct(triage, { kind: "close" });
      await workspaceClose(workflowId);
      onClose();
      if (thenReview) onOpenPrReview?.(target.project_id);
    } catch (e) { setFailure(asGuiError(e)); }
    finally { setBusy(false); }
  }, [workflowId, busy, tab, triage, onClose, onOpenPrReview, target.project_id]);

  const requestClose = useCallback(async () => {
    if (busy) return;
    try {
      if (workflowId && tab !== "threads") {
        await workspaceSwitchThreads(workflowId);
        setTab("threads");
      }
    } catch (e) {
      setFailure(asGuiError(e));
      return;
    }
    if (triageClose.current) triageClose.current();
    else void closeWorkspace();
  }, [workflowId, busy, tab, closeWorkspace]);

  useEffect(() => {
    if (!workflowId || !triage) return;
    const number = triage.review?.number ?? 0;
    const identity = triage.review ? `${triage.review.number}:${triage.review.head_sha}` : null;
    const changedPr = prIdentity.current !== null && identity !== prIdentity.current;
    const needsThreadRestore = changedPr && tabRef.current !== "threads";
    if (identity !== prIdentity.current) {
      if (changedPr) {
        ai.current?.reset();
        if (!needsThreadRestore) setTab("threads");
      }
      prIdentity.current = identity;
    }
    void (async () => {
      try {
        if (needsThreadRestore) {
          setBusy(true);
          await workspaceSwitchThreads(workflowId);
          setTab("threads");
        }
        await workspaceSelectPr(workflowId, number);
      } catch (e) { setFailure(asGuiError(e)); }
      finally { if (needsThreadRestore) setBusy(false); }
    })();
  }, [workflowId, triage?.review?.number, triage?.review?.head_sha]);

  return { workflowId, triage, setTriage, filesDirty, setFilesDirty, tab, busy, setBusy, triageBusy, setTriageBusy, failure, setFailure, error, setError, setAttempt, handoffPending, handoffGeneration, triageClose, switchTab, closeWorkspace, requestClose };
}

export type PrWorkspaceState = ReturnType<typeof usePrWorkspace>;
