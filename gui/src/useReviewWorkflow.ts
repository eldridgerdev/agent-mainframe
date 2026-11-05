import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  asGuiError,
  reviewAct,
  reviewBegin,
  reviewSnapshot,
  reviewTakeCompletion,
  type FeatureTarget,
  type ReviewAction,
  type ReviewView,
  type SessionTarget,
} from "./api";
import { SNAPSHOT_KEY } from "./workspaceSnapshot";

type ReviewWorkflowOptions = {
  onError: (error: unknown) => void;
  onOpenSession: (target: SessionTarget, draftPrompt?: string) => void;
  onNotice: (message: string) => void;
};

export function useReviewWorkflow({ onError, onOpenSession, onNotice }: ReviewWorkflowOptions) {
  const queryClient = useQueryClient();
  const [review, setReview] = useState<ReviewView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const actionPending = useRef(false);

  async function finish(workflowId: string) {
    const done = await reviewTakeCompletion(workflowId);
    setReview((current) => current?.workflow_id === workflowId ? null : current);
    if (!done) return;
    onNotice(done.message);
    if (done.handoff) onOpenSession(done.handoff.target, done.handoff.draft_prompt ?? undefined);
    void queryClient.invalidateQueries({ queryKey: SNAPSHOT_KEY });
  }

  useEffect(() => {
    if (!review || busy || (!review.ai.running && !review.preparation && review.check?.status !== "running")) return;
    let cancelled = false;
    let polling = false;
    const workflowId = review.workflow_id;
    const timer = window.setInterval(() => {
      if (polling || actionPending.current) return;
      polling = true;
      void reviewSnapshot(workflowId).then(async (next) => {
        // A null snapshot means the backend closed the review; collect its handoff once.
        if (next === null) { await finish(workflowId); return; }
        if (cancelled || actionPending.current) return;
        setReview((current) => current?.workflow_id === workflowId && next.workflow_id === workflowId
          && next.revision > current.revision ? next : current);
      }).catch(() => { /* Explicit actions report conflicts. */ }).finally(() => { polling = false; });
    }, 1000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [review?.workflow_id, review?.ai.running, review?.check?.status, review?.preparation?.operation_id, busy]);

  async function begin(target: FeatureTarget) {
    if (actionPending.current) return;
    actionPending.current = true;
    setBusy(true);
    setError(null);
    try { setReview(await reviewBegin(target)); }
    catch (cause) { onError(cause); }
    finally { actionPending.current = false; setBusy(false); }
  }

  async function refreshSyntax() {
    const workflowId = review?.workflow_id;
    if (!workflowId || actionPending.current) return;
    try {
      const next = await reviewSnapshot(workflowId);
      if (next) setReview((current) => current?.workflow_id === workflowId && next.revision >= current.revision ? next : current);
    } catch { /* The next action reports a closed or changed review. */ }
  }

  async function act(action: ReviewAction): Promise<boolean> {
    if (!review || actionPending.current) return false;
    actionPending.current = true;
    setBusy(true);
    setError(null);
    try {
      const next = await reviewAct(review, action);
      if (next === null && action.kind === "complete") {
        await finish(review.workflow_id);
        return true;
      }
      setReview(next);
      return !(action.kind === "pause" && next !== null);
    } catch (cause) { setError(asGuiError(cause).message); return false; }
    finally { actionPending.current = false; setBusy(false); }
  }

  return { review, busy, error, begin, refreshSyntax, act };
}
