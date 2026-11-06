import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  asGuiError,
  learningAct,
  learningBegin,
  learningLaunchAgent,
  learningSnapshot,
  type FeatureTarget,
  type LearningAction,
  type LearningView,
  type SessionTarget,
} from "./api";
import { SNAPSHOT_KEY } from "./workspaceSnapshot";

type LearningWorkflowOptions = {
  onError: (error: unknown) => void;
  onOpenSession: (target: SessionTarget, draftPrompt?: string) => void;
  onStartSession: (target: SessionTarget) => void;
  onNotice: (tone: "error" | "info", message: string) => void;
};

export function useLearningWorkflow({ onError, onOpenSession, onStartSession, onNotice }: LearningWorkflowOptions) {
  const queryClient = useQueryClient();
  const [learning, setLearning] = useState<LearningView | null>(null);
  const [busy, setBusy] = useState(false);
  const [approval, setApproval] = useState<{ qaId: string; message: string } | null>(null);
  const actionPending = useRef(false);

  useEffect(() => {
    if (!learning || busy) return;
    let cancelled = false;
    const workflowId = learning.workflow_id;
    const timer = window.setInterval(() => {
      void learningSnapshot().then((next) => {
        if (cancelled) return;
        setLearning((current) => {
          if (current?.workflow_id !== workflowId) return current;
          if (!next) return null;
          return next.workflow_id === current.workflow_id && next.revision >= current.revision ? next : current;
        });
      }).catch(() => { /* Explicit actions report deleted/stale targets. */ });
    }, 1000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [learning?.workflow_id, busy]);

  async function begin(target: FeatureTarget) {
    if (actionPending.current) return;
    actionPending.current = true;
    setBusy(true);
    try { setLearning(await learningBegin(target)); }
    catch (error) { onError(error); }
    finally { actionPending.current = false; setBusy(false); }
  }

  async function act(action: LearningAction): Promise<boolean> {
    if (!learning || actionPending.current) return false;
    actionPending.current = true;
    setBusy(true);
    try {
      setLearning(await learningAct(learning, action));
      if (action.kind === "keep_todo") void queryClient.invalidateQueries({ queryKey: ["todos"] });
      return true;
    } catch (error) { onError(error); return false; }
    finally { actionPending.current = false; setBusy(false); }
  }

  async function launch(qaId: string, approved = false) {
    if (!learning || actionPending.current) return;
    actionPending.current = true;
    setBusy(true);
    try {
      const handoff = await learningLaunchAgent(learning, qaId, approved);
      setLearning(null);
      setApproval(null);
      onOpenSession(handoff.target, handoff.draft_prompt ?? undefined);
      if (handoff.notice) onNotice("error", handoff.notice);
      if (handoff.info) onNotice("info", handoff.info);
      if (handoff.start_required) onStartSession(handoff.target);
      void queryClient.invalidateQueries({ queryKey: SNAPSHOT_KEY });
    } catch (error) {
      const guiError = asGuiError(error);
      if (guiError.kind === "needs_approval") setApproval({ qaId, message: guiError.message });
      else { setApproval(null); onError(error); }
    } finally { actionPending.current = false; setBusy(false); }
  }

  return { learning, busy, approval, begin, act, launch, cancelApproval: () => setApproval(null) };
}
