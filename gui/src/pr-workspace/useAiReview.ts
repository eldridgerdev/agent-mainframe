import { useEffect, useRef, useState } from "react";
import { asGuiError } from "../api";
import { preselectedTarget, targetKey } from "../FixDestinationPicker";
import type { PrFixHandoff, PrFixTarget } from "../prTriageApi";
import { workspaceAiAct, workspaceAiSnapshot, workspaceFixTargets, workspacePrepareFix, type WorkspaceAi } from "../prWorkspaceApi";
import type { AiLifecycle, PrWorkspaceState } from "./usePrWorkspace";

/** The AI review tab's state: the run, its findings and the batch fix. It lives with the workspace, not the
 * tab, so a run keeps reporting while another tab is open. */
export function useAiReview({ workspace, onHandoff, onStatusChange }: { workspace: PrWorkspaceState; onHandoff?: (handoff: PrFixHandoff) => void; onStatusChange: (status: { running: boolean; prNumber: number | null; completed: boolean }) => void }) {
  const { workflowId, triage, busy, setBusy, setFailure, setError, handoffPending, handoffGeneration } = workspace;
  const [ai, setAi] = useState<WorkspaceAi | null>(null);
  const [selectedFindings, setSelectedFindings] = useState<Set<number>>(new Set());
  const [fixTarget, setFixTarget] = useState<PrFixTarget | null>(null);
  const fixTouched = useRef(false);
  const [fixTargets, setFixTargets] = useState<PrFixTarget[]>([]);
  const [dedicatedOpen, setDedicatedOpen] = useState(false);
  const [companionOpen, setCompanionOpen] = useState(false);
  // A different workflow or PR starts with a clean choice: no stale target, no carried-over "touched" flag.
  useEffect(() => {
    fixTouched.current = false;
    setFixTarget(null);
    setFixTargets([]);
  }, [workflowId, triage?.review?.number]);
  // An existing companion session is offered up front; nothing is sent until the user prepares and confirms.
  useEffect(() => {
    if (fixTouched.current || fixTarget) return;
    const offered = preselectedTarget(fixTargets);
    if (offered) setFixTarget(offered);
  }, [fixTargets, fixTarget]);
  const handoffContext = useRef("");
  handoffContext.current = JSON.stringify([workflowId, ai?.result_revision, triage?.review?.number, triage?.review?.head_sha, fixTarget, fixTargets, [...selectedFindings].sort((a, b) => a - b)]);
  const [publishConfirm, setPublishConfirm] = useState(false);
  const [editing, setEditing] = useState<{ index: number; text: string } | null>(null);
  const aiLocalStartedAt = useRef<number | null>(null);
  const [clockNow, setClockNow] = useState(() => Date.now());

  // The lifecycle loads the AI view when its tab opens and forgets it when the PR changes.
  const lifecycle: AiLifecycle = {
    load: async (id) => {
      setAi(await workspaceAiSnapshot(id));
      setFixTargets(await workspaceFixTargets(id));
    },
    reset: () => {
      setAi(null);
      setSelectedFindings(new Set());
      setPublishConfirm(false);
    },
  };

  useEffect(() => {
    if (!workflowId || !ai?.running || busy) return;
    let live = true;
    let timer = 0;
    const poll = async () => {
      const startedAt = performance.now();
      try {
        const next = await workspaceAiSnapshot(workflowId);
        if (live) setAi(next);
      } catch (e) {
        if (live) setFailure(asGuiError(e));
      } finally {
        // Schedule after completion so a slow Tauri command cannot accumulate
        // overlapping snapshots and stale responses.
        if (live) timer = window.setTimeout(poll, Math.max(700, 1400 - (performance.now() - startedAt)));
      }
    };
    timer = window.setTimeout(poll, 700);
    return () => { live = false; window.clearTimeout(timer); };
  }, [workflowId, ai?.running, busy]);

  useEffect(() => {
    if (!ai?.running) {
      aiLocalStartedAt.current = null;
      return;
    }
    if (aiLocalStartedAt.current === null) {
      aiLocalStartedAt.current = Date.now() - ai.elapsed_seconds * 1000;
    }
    const timer = window.setInterval(() => setClockNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [ai?.running]);

  useEffect(() => {
    const hasResult = !!ai && !ai.running && (!!ai.summary || !!ai.error || ai.findings.length > 0);
    onStatusChange({ running: !!ai?.running, prNumber: ai?.pr_number ?? null, completed: hasResult });
  }, [ai?.running, ai?.pr_number, ai?.summary, ai?.error, ai?.findings.length, onStatusChange]);

  const aiAct = async (kind: "start" | "cancel" | "confirm_precall" | "cancel_precall" | "post") => {
    if (!workflowId || busy) return;
    setBusy(true);
    if (kind === "start") setSelectedFindings(new Set());
    try {
      setAi(await workspaceAiAct(workflowId, { kind }));
      setError(null);
      if (kind === "post") { setPublishConfirm(false); setSelectedFindings(new Set()); }
    }
    catch (e) { setFailure(asGuiError(e)); }
    finally { setBusy(false); }
  };

  // A skip or edit is bound to the exact result and PR head on screen; the
  // engine refuses anything stale instead of retargeting it.
  const changeFinding = async (action: { kind: "toggle_skip"; index: number } | { kind: "edit_body"; index: number; body: string }) => {
    if (!workflowId || busy || !ai?.result_revision) return;
    setBusy(true);
    try {
      setAi(await workspaceAiAct(workflowId, { ...action, result_revision: ai.result_revision, head_sha: ai.head_sha }));
      setSelectedFindings(new Set());
      setEditing(null);
      setError(null);
    }
    catch (e) { setFailure(asGuiError(e)); }
    finally { setBusy(false); }
  };

  // An open edit belongs to one result. Other finding actions are disabled while it
  // is open, so only a new run or a change from elsewhere can reach this.
  useEffect(() => { setEditing(null); }, [ai?.result_revision]);

  const publishableFindings = ai?.findings.filter((finding) => !finding.skipped && !finding.published) ?? [];
  const fixableFindingIndices = ai?.findings.flatMap((finding, index) => !finding.skipped && !finding.published ? [index] : []) ?? [];
  const selectedFixable = [...selectedFindings].filter((index) => fixableFindingIndices.includes(index)).sort((a, b) => a - b);
  const fixTargetStale = !!fixTarget && !fixTargets.some((item) => targetKey(item) === targetKey(fixTarget) && item.destination_revision === fixTarget.destination_revision && !item.stopped);
  const allFixableSelected = fixableFindingIndices.length > 0 && fixableFindingIndices.every((index) => selectedFindings.has(index));
  const toggleAllFixableFindings = () => setSelectedFindings((current) => {
    const next = new Set(current);
    for (const index of fixableFindingIndices) {
      if (allFixableSelected) next.delete(index);
      else next.add(index);
    }
    return next;
  });
  const prepareFix = async (indices: number[]) => {
    if (!workflowId || !ai?.result_revision || !fixTarget || !onHandoff || !indices.length || handoffPending.current) return;
    const chosen = fixTargets.find((item) => targetKey(item) === targetKey(fixTarget) && item.destination_revision === fixTarget.destination_revision && !item.stopped);
    if (!chosen) { setError("The selected agent session is unavailable or changed; choose a target again."); return; }
    const generation = ++handoffGeneration.current;
    const context = handoffContext.current;
    handoffPending.current = true;
    setBusy(true);
    try {
      const handoff = await workspacePrepareFix({ workflow_id: workflowId, result_revision: ai.result_revision, finding_indices: indices, target: fixTarget.target, destination_revision: fixTarget.destination_revision, destination_kind: fixTarget.destination_kind });
      if (handoffGeneration.current === generation) {
        if (handoffContext.current === context) { setError(null); onHandoff(handoff); }
        else setError("The findings or fix agent changed while preparing; try again.");
      }
    } catch (e) {
      if (handoffGeneration.current === generation && handoffContext.current === context) {
        setFailure(asGuiError(e));
        try {
          const targets = await workspaceFixTargets(workflowId);
          if (handoffGeneration.current === generation && handoffContext.current === context) setFixTargets(targets);
        } catch { /* preserve the selected target */ }
      }
    } finally {
      handoffPending.current = false;
      if (handoffGeneration.current === generation) setBusy(false);
    }
  };

  const aiElapsedSeconds = ai?.running && aiLocalStartedAt.current !== null
    ? Math.max(ai.elapsed_seconds, Math.floor((clockNow - aiLocalStartedAt.current) / 1000))
    : ai?.elapsed_seconds ?? 0;

  return { ai, lifecycle, selectedFindings, setSelectedFindings, fixTarget, setFixTarget, fixTouched, fixTargets, setFixTargets, dedicatedOpen, setDedicatedOpen, companionOpen, setCompanionOpen, publishConfirm, setPublishConfirm, editing, setEditing, aiAct, changeFinding, publishableFindings, fixableFindingIndices, selectedFixable, fixTargetStale, allFixableSelected, toggleAllFixableFindings, prepareFix, aiElapsedSeconds };
}

export type AiReviewState = ReturnType<typeof useAiReview>;
