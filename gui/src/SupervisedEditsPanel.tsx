import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { DiffOptions, FeatureTarget, asGuiError } from "./api";
import { Hunk } from "./DiffPanel";
import { EmptyState, Field, Icon, Modal, Spinner } from "./ui";
import {
  MAX_FEEDBACK_CHARS,
  SupervisedEdit,
  SupervisedEditDecision,
  SupervisedEditsView,
  supervisedEditRespond,
  supervisedEditsLoad,
} from "./supervisedEditsApi";

export { PENDING_EDITS_KEY, usePendingEdits } from "./usePendingEdits";
import { PENDING_EDITS_KEY } from "./usePendingEdits";

type DecisionKind = SupervisedEditDecision["kind"];

const CONFIRM: Record<DecisionKind, { title: string; button: string; tone: string }> = {
  approve: { title: "Approve this edit?", button: "Send approval", tone: "btn-primary" },
  reject: { title: "Reject this edit?", button: "Send rejection", tone: "btn-danger" },
  cancel: { title: "Cancel this edit?", button: "Send cancellation", tone: "btn-warning" },
};

const TOOL_LABEL: Record<string, string> = { edit: "Edit", write: "Write" };

function requestedAt(edit: SupervisedEdit) {
  if (edit.requested_at == null) return null;
  return new Date(edit.requested_at * 1000).toLocaleTimeString();
}

/** Vibeless mode's per-edit approval. Every answer is confirmed first and
 * names the exact revision the reviewer saw; the backend refuses it when the
 * edit changed, was answered elsewhere or the agent stopped waiting. */
export type SupervisedEditsPanelHandle = {
  requestSwitch: (proceed: () => void) => void;
};

const SupervisedEditsPanel = forwardRef<SupervisedEditsPanelHandle, {
  target: FeatureTarget;
  onClose: () => void;
  /** Called after an answer is delivered, so navigation counts refresh. */
  onAnswered?: (message: string) => void;
  initialEditId?: string;
  moreWaiting?: number;
  autoOpen?: boolean;
  onAutoOpenChange?: (enabled: boolean) => void;
}>(({ target, onClose, onAnswered, initialEditId, moreWaiting = 0, autoOpen, onAutoOpenChange }, ref) => {
  const container = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = () => requestClose();
  const queryClient = useQueryClient();
  const [context, setContext] = useState<DiffOptions["context"]>("standard");
  const [split, setSplit] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(initialEditId ?? null);
  const [feedback, setFeedback] = useState<Record<string, string>>({});
  const feedbackPaths = useRef<Record<string, string>>({});
  const [confirm, setConfirm] = useState<{ id: string; revision: string; kind: DecisionKind } | null>(null);
  const [sending, setSending] = useState(false);
  const inFlight = useRef(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [lost, setLost] = useState<string | null>(null);
  const [discardPrompt, setDiscardPrompt] = useState<{ proceed: () => void; switching: boolean } | null>(null);
  const answered = useRef(new Set<string>());
  const lastSelected = useRef<SupervisedEdit | null>(null);

  const queryKey = ["supervised-edits", target.project_id, target.feature_id, context];
  const query = useQuery({
    queryKey,
    queryFn: () => supervisedEditsLoad(target, context),
    refetchInterval: 1_500,
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnMount: "always",
  });
  // A new popup must read the current hook files before showing a cached
  // review. Otherwise the previous popup's answered edit can flash here and
  // produce a misleading "answered elsewhere" notice for the wrong file.
  const view = query.isFetchedAfterMount ? query.data : undefined;
  const edits = view?.edits ?? [];
  const edit = edits.find((candidate) => candidate.id === selectedId) ?? edits[0];

  // Own keyboard focus while reviewing, then return to the same terminal or
  // control. Capture Escape so it cannot dismiss another workflow underneath.
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    container.current?.querySelector<HTMLElement>("[role=dialog]")?.focus();
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        close.current();
      } else if (event.key === "Tab") {
        const controls = Array.from(container.current?.querySelectorAll<HTMLElement>(
          "button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled)",
        ) ?? []);
        const index = controls.indexOf(document.activeElement as HTMLElement);
        if (index < 0 || (!event.shiftKey && index === controls.length - 1) || (event.shiftKey && index === 0)) {
          event.preventDefault();
          controls[event.shiftKey ? controls.length - 1 : 0]?.focus();
        }
      }
    };
    document.addEventListener("keydown", key, true);
    return () => {
      document.removeEventListener("keydown", key, true);
      if (previous?.isConnected) previous.focus();
    };
  }, []);

  const [readyEdit, setReadyEdit] = useState<string | null>(null);
  const holdIdentity = edit ? `${edit.id}:${edit.revision}` : null;
  const holdSecs = view?.popup_hold_secs ?? 1.5;
  useEffect(() => {
    setReadyEdit(null);
    if (!holdIdentity) return;
    const delay = Number.isFinite(holdSecs) ? Math.max(0, holdSecs) * 1000 : 1500;
    const timer = window.setTimeout(() => setReadyEdit(holdIdentity), delay);
    return () => window.clearTimeout(timer);
  }, [holdIdentity, holdSecs]);
  const holding = holdSecs > 0 && holdIdentity !== null && readyEdit !== holdIdentity;

  // The edit being read can vanish (answered in another window, or its agent
  // stopped) or change underneath the reviewer. Say so, and never let an
  // open confirmation carry over to a different revision.
  useEffect(() => {
    if (!view) return;
    const previous = lastSelected.current;
    if (previous && !edits.some((candidate) => candidate.id === previous.id)) {
      if (!answered.current.has(previous.id)) {
        const draft = feedback[previous.id]?.trim();
        setLost(`The edit to ${previous.path} is no longer waiting for review: it was answered elsewhere or its agent stopped.`
          + (draft ? " Your unsent feedback was not delivered." : ""));
      }
      setFeedback((current) => {
        if (!(previous.id in current)) return current;
        const next = { ...current };
        delete next[previous.id];
        return next;
      });
    }
    if (confirm && !edits.some((candidate) => candidate.id === confirm.id && candidate.revision === confirm.revision)) {
      setConfirm(null);
      if (edits.some((candidate) => candidate.id === confirm.id)) {
        setNotice("This edit changed while you were confirming. Review it again before answering.");
      }
    }
    lastSelected.current = edit ?? null;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view]);

  const draft = edit ? feedback[edit.id] ?? "" : "";
  const unsentDrafts = Object.entries(feedback)
    .filter(([, text]) => text.trim() !== "")
    .map(([id]) => feedbackPaths.current[id] ?? "a pending edit");
  const blocked = !edit || edit.answered || edit.unavailable !== null || sending;

  function requestLeave(proceed: () => void, switching = false) {
    if (inFlight.current) {
      if (switching) setNotice("An answer is being sent. Wait for its result before switching reviews.");
      return;
    }
    if (unsentDrafts.length > 0) setDiscardPrompt({ proceed, switching });
    else proceed();
  }

  function requestClose() { requestLeave(onClose); }

  useImperativeHandle(ref, () => ({ requestSwitch: (proceed) => requestLeave(proceed, true) }));

  function select(id: string) {
    setSelectedId(id);
    setConfirm(null);
    setError(null);
    lastSelected.current = edits.find((candidate) => candidate.id === id) ?? null;
  }

  async function send() {
    if (!confirm || !edit || inFlight.current || holding) return;
    if (edit.id !== confirm.id || edit.revision !== confirm.revision) {
      setConfirm(null);
      setNotice("This edit changed while you were confirming. Review it again before answering.");
      return;
    }
    inFlight.current = true;
    setSending(true);
    setError(null);
    const decision: SupervisedEditDecision = confirm.kind === "reject"
      ? { kind: "reject", feedback: draft.trim() }
      : { kind: confirm.kind };
    try {
      const outcome = await supervisedEditRespond(target, confirm, decision);
      answered.current.add(confirm.id);
      queryClient.setQueryData<SupervisedEditsView>(queryKey, outcome.view);
      setFeedback((current) => {
        const next = { ...current };
        delete next[confirm.id];
        return next;
      });
      setNotice(`${outcome.message}. The agent continues once its hook reads the answer.`);
      setLost(null);
      setConfirm(null);
      void queryClient.invalidateQueries({ queryKey: PENDING_EDITS_KEY });
      if (!Object.entries(feedback).some(([id, text]) => id !== confirm.id && text.trim())) onAnswered?.(outcome.message);
    } catch (err) {
      const failure = asGuiError(err);
      setError(failure.message);
      setConfirm(null);
      if (failure.kind === "conflict" || failure.kind === "not_found") void query.refetch();
    } finally {
      inFlight.current = false;
      setSending(false);
    }
  }

  const newFile = edit?.is_new_file || edit?.diff?.status === "added";
  const layoutSplit = split && !newFile;

  return <div ref={container} className="supervised-popup"><Modal label="Supervised edits" title={view ? `Supervised edits · ${view.feature_name}` : "Supervised edits"}
    subtitle="Vibeless agents wait for your answer before writing each file change." size="xl"
    onClose={requestClose} dismissable={!sending}
    footer={<button className="btn btn-secondary" disabled={sending} onClick={requestClose}>Close</button>}>
    {onAutoOpenChange && <label className="switch-row">
      <input type="checkbox" checked={autoOpen} onChange={(event) => onAutoOpenChange(event.target.checked)} />
      <span>Automatically open waiting edits</span>
    </label>}
    {moreWaiting > 0 && <p role="status">{moreWaiting} more waiting</p>}
    {holding && <p role="status" className="callout callout-warning">Review hold · answers available after {holdSecs} seconds.</p>}
    <div className="diff-controls">
      <Field label="Layout"><select value={layoutSplit ? "split" : "unified"} disabled={newFile}
        onChange={(event) => setSplit(event.target.value === "split")}>
        <option value="unified">Unified</option><option value="split">Side by side</option>
      </select></Field>
      <Field label="Context"><select value={context} onChange={(event) => setContext(event.target.value as DiffOptions["context"])}>
        <option value="standard">3 lines</option><option value="expanded">10 lines</option><option value="full">Whole file</option>
      </select></Field>
      <button className="btn btn-secondary btn-sm" onClick={() => void query.refetch()} disabled={query.isFetching}>Refresh</button>
    </div>
    {!view && query.isFetching && <p role="status"><Spinner /> Loading pending edits…</p>}
    {query.error && <p role="alert">{asGuiError(query.error).message}</p>}
    {notice && <p role="status" className="callout callout-accent">{notice}</p>}
    {lost && <p role="status" className="callout callout-warning">{lost}</p>}
    {error && <p role="alert" className="callout callout-warning">{error}</p>}
    {discardPrompt && <div role="alertdialog" aria-label="Discard unsent feedback" className="callout callout-warning supervised-confirm">
      <p>Discard unsent feedback for {unsentDrafts.join(", ")}? Leaving does not answer the agent; the edit keeps waiting.</p>
      <div className="supervised-actions">
        <button className="btn btn-ghost" onClick={() => setDiscardPrompt(null)}>Keep editing</button>
        <button className="btn btn-warning" disabled={sending} onClick={() => {
          if (!inFlight.current) { discardPrompt.proceed(); setDiscardPrompt(null); }
        }}>{discardPrompt.switching ? "Discard and switch" : "Discard and close"}</button>
      </div>
    </div>}
    {view && !query.error && (edits.length === 0
      ? <EmptyState icon="check" title="No edits are waiting for review">
        Edits a Vibeless agent asks to write appear here. Requests already delivered to a running AMF TUI are answered there.
      </EmptyState>
      : <div className="diff-reader">
        <nav className="diff-files" aria-label="Pending edits">{edits.map((candidate) => <button key={candidate.id}
          className={`diff-file ${candidate.id === edit?.id ? "diff-file-selected" : ""}`} aria-pressed={candidate.id === edit?.id}
          onClick={() => select(candidate.id)}>
          <span>{candidate.path}</span>
          <small>{TOOL_LABEL[candidate.tool] ?? (candidate.tool || "Change")}
            {" · "}{candidate.answered ? "Answer sent" : candidate.unavailable ? "Not answerable" : "Waiting"}</small>
        </button>)}</nav>
        {edit && <section className="diff-content" aria-label="Pending edit">
          <div className="diff-file-header">
            <strong>{edit.path}</strong>
            <span>{TOOL_LABEL[edit.tool] ?? edit.tool}{newFile ? " · new file" : ""}</span>
            {requestedAt(edit) && <span>Requested {requestedAt(edit)}</span>}
          </div>
          {edit.agent_reason && <p className="supervised-reason"><strong>Agent's reason:</strong> {edit.agent_reason}</p>}
          {edit.answered && <p role="status" className="callout callout-accent">Answer sent. Waiting for the agent to pick it up.</p>}
          {edit.unavailable && <p role="status" className="callout callout-warning">{edit.unavailable}</p>}
          <div className="diff-code">
            {edit.diff_error && <p role="status">Diff preview unavailable: {edit.diff_error}</p>}
            {edit.diff
              ? edit.diff.is_binary ? <p>Binary file; no text diff is available.</p>
                : edit.diff.hunks.length === 0 ? <pre>{edit.diff.patch || "No textual changes."}</pre>
                : edit.diff.hunks.map((hunk, index) => <Hunk key={index} hunk={hunk} split={layoutSplit} />)
              : <>
                {edit.old_snippet && <><h4>Removed</h4><pre className="supervised-snippet diff-removed">{edit.old_snippet}</pre></>}
                {edit.new_snippet && <><h4>Added</h4><pre className="supervised-snippet diff-added">{edit.new_snippet}</pre></>}
              </>}
          </div>
          <Field label="Feedback for the agent"
            hint={edit.effects.feedback_reaches_agent
              ? `Sent with Reject (optional, ${draft.length}/${MAX_FEEDBACK_CHARS}).`
              : `OpenCode does not forward rejection feedback (${draft.length}/${MAX_FEEDBACK_CHARS}).`}>
            <textarea aria-label="Feedback for the agent" rows={2} maxLength={MAX_FEEDBACK_CHARS} value={draft} disabled={blocked}
              onChange={(event) => { const text = event.target.value; feedbackPaths.current[edit.id] = edit.path; setFeedback((current) => ({ ...current, [edit.id]: text })); }} />
          </Field>
          {confirm && confirm.id === edit.id
            ? <div role="alertdialog" aria-label="Confirm answer" className="callout callout-warning supervised-confirm">
              <p><strong>{CONFIRM[confirm.kind].title}</strong> {edit.effects[confirm.kind]}</p>
              {confirm.kind === "reject" && edit.effects.feedback_reaches_agent && <p>
                {draft.trim() ? <>Feedback: <q>{draft.trim()}</q></> : "No feedback will be sent."}
              </p>}
              <div className="supervised-actions">
                <button className="btn btn-ghost" disabled={sending} onClick={() => setConfirm(null)}>Back</button>
                <button className={`btn ${CONFIRM[confirm.kind].tone}`} disabled={sending || holding} onClick={() => void send()}>
                  {sending && <Spinner />}{CONFIRM[confirm.kind].button}
                </button>
              </div>
            </div>
            : <div className="supervised-actions">
              {(["approve", "reject", "cancel"] as const).map((kind) => <button key={kind}
                className={`btn ${kind === "approve" ? "btn-primary" : kind === "reject" ? "btn-danger" : "btn-secondary"}`}
                disabled={blocked || holding} onClick={() => { setError(null); setNotice(null); setConfirm({ id: edit.id, revision: edit.revision, kind }); }}>
                {kind === "approve" && <Icon name="check" size={12} />}
                {kind === "approve" ? "Approve edit" : kind === "reject" ? "Reject edit" : "Cancel edit"}
              </button>)}
            </div>}
        </section>}
      </div>)}
  </Modal></div>;
});

export default SupervisedEditsPanel;
