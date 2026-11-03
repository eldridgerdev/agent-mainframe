import { useEffect, useMemo, useRef, useState } from "react";
import { AgentSlug, ModeSlug, asGuiError } from "./api";
import {
  CompanionCatalog, PrDestinationChoice, PrDestinationOperation, PrDestinationPrepared, PrDestinationPreview, PrDestinationWorkflow,
  companionCatalog, destinationCancel, destinationPrepare, destinationPreview, destinationStatus,
} from "./prDestinationApi";
import { Field, Spinner } from "./ui";

const INPUT_DEBOUNCE_MS = 250;
const POLL_MS = 300;
/** Consecutive failed status polls tolerated before the operation is treated as lost. */
const POLL_RETRIES = 3;

const PHASE_LABEL: Record<PrDestinationOperation["phase"], string> = {
  starting: "Starting", fetching: "Fetching the PR head from the remote", creating_worktree: "Creating the separate checkout",
  running_hook: "Running the setup script", registering: "Registering the stopped feature", done: "Finished",
};

/** The operation's final answer in the shape the dedicated-session form hands its caller. */
export function preparedFrom(operation: PrDestinationOperation): PrDestinationPrepared {
  return {
    status: "created", operation_id: operation.operation_id, created_target: operation.created_target,
    destination_revision: operation.destination_revision, destination_kind: operation.destination_kind,
    stopped: true, message: operation.message,
  };
}

/** The creation produced a feature even if it did not finish cleanly (a cancel
 * that arrived after the worktree existed keeps and registers it). */
const producedFeature = (op: PrDestinationOperation) => op.created_target !== null && (op.state === "created" || op.state === "cancelled");

/** Create a NEW triage feature (its own worktree and branch, seeded from the PR
 * head), stopped. This is a separate screen from sending a fix: it only creates.
 * A read-only preview comes first; "Create (stopped)" repeats every check on the
 * backend and starts a cancellable background operation that never starts the
 * agent, sends a prompt, pushes, or posts to GitHub. The caller gets the created
 * target back and continues into its existing fix draft or batch preview. */
export default function CompanionFeatureForm({ workflow, expected, onCreated, onClose }: {
  workflow: PrDestinationWorkflow;
  /** The PR the surrounding panel is showing; a different live PR is a conflict. */
  expected?: { pr_number: number; head_sha: string } | null;
  onCreated: (result: PrDestinationPrepared) => void;
  onClose: () => void;
}) {
  const [catalog, setCatalog] = useState<CompanionCatalog | null>(null);
  const [branch, setBranch] = useState("");
  const [agent, setAgent] = useState<AgentSlug | "">("");
  const [mode, setMode] = useState<ModeSlug>("vibeless");
  const [review, setReview] = useState(false);
  const [chrome, setChrome] = useState(false);
  const [hookChoice, setHookChoice] = useState("");
  const [committed, setCommitted] = useState<string | null>(null);
  const [preview, setPreview] = useState<{ key: string; value: PrDestinationPreview } | null>(null);
  const [approved, setApproved] = useState(false);
  const [needsApproval, setNeedsApproval] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [operationId, setOperationId] = useState<string | null>(null);
  const [operation, setOperation] = useState<PrDestinationOperation | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [operationLost, setOperationLost] = useState(false);
  const [reload, setReload] = useState(0);
  const submitting = useRef(false);
  const previewSeq = useRef(0);
  const reported = useRef(false);
  const live = useRef({ operationId: null as string | null, settled: false });

  useEffect(() => {
    let live = true;
    companionCatalog(workflow).then((next) => {
      if (!live) return;
      if (expected && (expected.pr_number !== next.identity.pr_number || expected.head_sha !== next.identity.head_sha)) {
        setError("The PR changed since this panel loaded. Refresh the panel, then reopen this form.");
        return;
      }
      setCatalog(next);
      setBranch((current) => current || next.suggested_branch);
      setMode((current) => current === "vibeless" ? next.default_mode : current);
      setAgent((current) => current && next.agents.some((a) => a.agent === current && !a.unavailable) ? current
        : (next.agents.find((a) => a.agent === next.preferred && !a.unavailable) ?? next.agents.find((a) => !a.unavailable))?.agent ?? "");
    }).catch((err) => { if (live) setError(asGuiError(err).message); });
    return () => { live = false; };
  }, [workflow.kind, workflow.workflow_id, reload]);

  const choice = useMemo<PrDestinationChoice | null>(() => agent && branch.trim() ? {
    kind: "companion_feature",
    settings: { branch: branch.trim(), agent, mode, review, enable_chrome: chrome },
    hook_choice: hookChoice || null,
  } : null, [agent, branch, mode, review, chrome, hookChoice]);
  const inputKey = JSON.stringify(choice);
  useEffect(() => {
    const timer = window.setTimeout(() => setCommitted(inputKey), INPUT_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [inputKey]);

  const running = operationId !== null && !operationLost && (operation === null || operation.state === "running");
  const finished = operation !== null && operation.state !== "running";
  useEffect(() => {
    if (!catalog || !choice || committed !== inputKey || operationId) return;
    const seq = ++previewSeq.current;
    setPreview(null);
    setApproved(false);
    setNeedsApproval(null);
    destinationPreview(workflow, catalog.identity, choice)
      .then((value) => { if (previewSeq.current === seq) { setPreview({ key: inputKey, value }); setError(null); } })
      .catch((err) => { if (previewSeq.current === seq) setError(asGuiError(err).message); });
  }, [catalog, committed, reload]);

  // Follow the background operation until it settles.
  useEffect(() => {
    if (!operationId) return;
    let alive = true;
    let failures = 0;
    const timer = window.setInterval(() => {
      destinationStatus(operationId).then((view) => {
        if (!alive) return;
        failures = 0;
        setOperation(view);
        if (view.state === "running") return;
        window.clearInterval(timer);
        live.current.settled = true;
        setCancelling(false);
        if (producedFeature(view) && !reported.current) {
          reported.current = true;
          onCreated(preparedFrom(view));
        }
      }).catch((err) => {
        if (!alive) return;
        const failure = asGuiError(err);
        failures += 1;
        // An untracked operation will never answer; a transient error may.
        if (failure.kind !== "not_found" && failures < POLL_RETRIES) return;
        window.clearInterval(timer);
        live.current.settled = true;
        setOperationLost(true);
        setCancelling(false);
        setError(`AMF lost track of this creation: ${failure.message.replace(/[.\s]+$/, "")}. The feature may still have been created, so check the feature list before trying again.`);
      });
    }, POLL_MS);
    return () => { alive = false; window.clearInterval(timer); };
  }, [operationId]);

  // Leaving the form while a creation runs must not strand it: ask it to stop
  // (idempotent, and whatever already exists is kept and registered).
  useEffect(() => { live.current.operationId = operationId; live.current.settled = false; }, [operationId]);
  useEffect(() => () => {
    const { operationId: id, settled } = live.current;
    if (id && !settled) void destinationCancel(id).catch(() => undefined);
  }, []);

  const current = preview && preview.key === inputKey ? preview.value : null;
  const companion = current?.companion ?? null;
  // A setup hook runs files from the PR checkout; without a confirmed `origin`
  // branch (a fork, or an unreachable remote) that needs an explicit yes.
  const hookAck = companion?.hook && companion.pr_branch_on_origin !== true
    ? `The setup script would run files from this PR's checkout, and ${companion.pr_branch_on_origin === false ? `the PR branch “${companion.pr_branch}” is not on the remote (for example a fork)` : "AMF could not confirm the PR branch on the remote"}. That code is not from your repository.`
    : null;
  const lateGate = needsApproval && !(hookAck && needsApproval.includes("setup script would run files")) ? needsApproval : null;
  const resourceGate = current?.resource_warning ?? lateGate;
  const gate = resourceGate || hookAck;
  const hookNeedsChoice = !!companion?.hook?.choice_required;
  const canSubmit = !!current && !!companion && !busy && !operationId && (!gate || approved) && !hookNeedsChoice;

  const submit = async () => {
    // A second click in the same tick must not start a second creation.
    if (submitting.current || !catalog || !choice || !current || !canSubmit) return;
    submitting.current = true;
    setBusy(true);
    try {
      const result = await destinationPrepare(workflow, catalog.identity, choice, current.revision, approved);
      setOperation(null);
      setOperationId(result.operation_id);
    } catch (err) {
      const failure = asGuiError(err);
      if (failure.kind === "needs_approval") setNeedsApproval(failure.message);
      else { setError(failure.message); setPreview(null); }
      submitting.current = false;
    } finally {
      setBusy(false);
    }
  };

  const cancel = async () => {
    if (!operationId || cancelling) return;
    setCancelling(true);
    try { setOperation(await destinationCancel(operationId)); } catch (err) { setError(asGuiError(err).message); setCancelling(false); }
  };

  // After a failure or cancel the form is editable again against a fresh preview.
  const again = () => {
    submitting.current = false;
    reported.current = false;
    setOperationId(null);
    setOperationLost(false);
    setOperation(null);
    setPreview(null);
    setError(null);
    setReload((n) => n + 1);
  };

  const unavailable = catalog?.agents.filter((a) => a.unavailable) ?? [];
  const locked = busy || (!!operationId && !operationLost);
  const unsupported = (m: CompanionCatalog["modes"][number]) => !!agent && m.unsupported_agents.includes(agent);
  const hook = catalog?.hook ?? null;
  return (
    <section className="review-editor companion-feature-form" aria-label="New triage feature">
      <h3>New triage feature</h3>
      <p>
        An isolated feature with its own separate checkout (worktree) and a new branch, started from <strong>this PR&apos;s head commit</strong> and
        linked to PR #{catalog?.identity.pr_number ?? expected?.pr_number ?? "?"}. The agent is created <strong>stopped</strong>: nothing runs
        and no prompt is sent; start it from its session tab. Nothing is pushed to the remote and nothing is posted to GitHub.
      </p>
      {error && !finished && <p className="callout callout-danger" role="alert">{error} <button type="button" className="btn btn-ghost btn-sm" disabled={locked} onClick={again}>Refresh</button></p>}
      {catalog && <>
        <Field label="Feature and branch name" hint={`Suggested from the PR branch “${catalog.pr_branch || catalog.source_branch}”. Always a new branch; an existing one is never reused or reset.`}>
          <input type="text" value={branch} maxLength={100} disabled={locked} onChange={(event) => setBranch(event.target.value)} />
        </Field>
        <Field label="Agent">
          <select value={agent} disabled={locked} onChange={(event) => setAgent(event.target.value as AgentSlug)}>
            {catalog.agents.map((a) => <option key={a.agent} value={a.agent} disabled={!!a.unavailable}>{a.label}{a.unavailable ? " (unavailable)" : ""}</option>)}
          </select>
        </Field>
        {unavailable.length > 0 && <p className="small muted">{unavailable.map((a) => `${a.label}: ${a.unavailable}`).join(" · ")}</p>}
        <Field label="Mode" hint={catalog.modes.find((m) => m.mode === mode)?.description}>
          <select value={mode} disabled={locked} onChange={(event) => setMode(event.target.value as ModeSlug)}>
            {catalog.modes.map((m) => <option key={m.mode} value={m.mode} disabled={unsupported(m)}>{m.label}{unsupported(m) ? " (not supported by this agent)" : ""}</option>)}
          </select>
        </Field>
        <label className="row small"><input type="checkbox" checked={review} disabled={locked} onChange={(event) => setReview(event.target.checked)} /><span>Review workflow</span></label>
        <label className="row small"><input type="checkbox" checked={chrome} disabled={locked} onChange={(event) => setChrome(event.target.checked)} /><span>Enable Chrome</span></label>
        {hook?.prompt_title && <Field label={hook.prompt_title} hint="The project's setup script asks this once, before the feature is added.">
          <select value={hookChoice} disabled={locked} onChange={(event) => setHookChoice(event.target.value)}>
            <option value="">Choose a setup option</option>
            {hook.options.map((option) => <option key={option} value={option}>{option}</option>)}
          </select>
        </Field>}
      </>}
      {!current && !error && catalog && !operationId && <p className="muted small" role="status">Checking what would be created…</p>}
      {companion && current && !operationId && <div aria-label="What will happen">
        <dl className="companion-facts small">
          <dt>Feature and branch</dt><dd>{companion.branch}</dd>
          <dt>Checkout</dt><dd><code>{companion.destination_path}</code></dd>
          <dt>Started from</dt><dd>PR head <code>{companion.base_short}</code>{companion.will_fetch ? " (fetched from the remote first)" : ""}</dd>
          <dt>PR branch (on the remote)</dt><dd>{companion.pr_branch || "unknown"}{companion.pr_branch_on_origin === true ? " · found" : companion.pr_branch_on_origin === false ? " · not found" : " · not checked"}</dd>
          <dt>Agent and mode</dt><dd>{companion.agent} · {companion.mode}{companion.review ? " · review" : ""}{companion.enable_chrome ? " · Chrome" : ""}</dd>
          {companion.hook && <><dt>Setup script</dt><dd><code>{companion.hook.script}</code>{companion.hook.choice ? ` · ${companion.hook.prompt_title ?? "choice"}: ${companion.hook.choice}` : ""}</dd></>}
        </dl>
        {companion.pr_branch_note && <p className={companion.pr_branch_on_origin === false ? "callout callout-danger" : "small muted"} role={companion.pr_branch_on_origin === false ? "alert" : undefined}>{companion.pr_branch_note}</p>}
        <ul>{current.consequences.map((line) => <li key={line}>{line}</li>)}</ul>
      </div>}
      {gate && !operationId && <label className="row small">
        <input type="checkbox" checked={approved} disabled={locked} onChange={(event) => setApproved(event.target.checked)} />
        <span>{resourceGate ? <>I approve despite the resource warning: {resourceGate}{hookAck ? ` ${hookAck}` : ""}</> : <>I understand and approve running the setup script: {hookAck}</>}</span>
      </label>}
      {operationId && !operationLost && <div className="callout callout-accent" role="status" aria-live="polite">
        {running && <p className="row"><Spinner /> <strong>{operation ? PHASE_LABEL[operation.phase] : "Starting"}…</strong></p>}
        <p>{operation?.message ?? "Creating in the background. Nothing is started."}</p>
        {operation?.retained_path && <p className="small">Checkout: <code>{operation.retained_path}</code></p>}
        {operation?.cancel_requested && running && <p className="small">Cancelling. Anything already created is kept, and nothing is deleted automatically.</p>}
      </div>}
      {finished && operation.state !== "created" && !producedFeature(operation) && <p className={operation.state === "failed" ? "callout callout-danger" : "callout"} role={operation.state === "failed" ? "alert" : "status"}>
        {operation.state === "failed" ? "Creation failed. " : "Creation cancelled. "}{operation.retained_path ? "The checkout was kept; see its path above." : "Nothing was left behind."}
      </p>}
      <div className="row">
        {!operationId && <button type="button" className="btn btn-primary" disabled={!canSubmit} onClick={() => void submit()}>{busy ? "Working…" : "Create (stopped)"}</button>}
        {running && <button type="button" className="btn btn-secondary" disabled={cancelling || operation?.cancel_requested} onClick={() => void cancel()}>{cancelling || operation?.cancel_requested ? "Cancelling…" : "Cancel creation"}</button>}
        {finished && operation.state !== "created" && !producedFeature(operation) && <button type="button" className="btn btn-secondary" onClick={again}>Edit and try again</button>}
        <button type="button" className="btn btn-secondary" disabled={busy || running} title={running ? "Cancel the creation first" : undefined} onClick={onClose}>{finished || operationLost ? "Close" : "Cancel"}</button>
      </div>
      {running && <p className="field-hint">Cancel the creation first to close this form.</p>}
    </section>
  );
}
