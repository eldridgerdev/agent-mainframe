import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { AgentSlug, FeatureTarget, HarnessInfo, asGuiError, projectHarnessChoices } from "./api";
import {
  ForkOutcome, ForkPhase, ForkPreview, forkCancel, forkConfirm, forkPrepare, forkStatus,
} from "./forkApi";
import { Field, Icon, Modal, Spinner, Switch } from "./ui";

const PHASE_LABEL: Record<ForkPhase, string> = {
  checking: "Checking the source is unchanged…",
  creating_checkout: "Creating the worktree at the source HEAD…",
  transferring: "Copying working changes…",
  registering: "Registering the stopped feature…",
  hook: "Running the worktree hook…",
  starting: "Starting the agent…",
};

const KIND_LABEL = { file: "file", executable: "executable", symlink: "link", deleted: "deleted" } as const;
const SHOWN_CHANGES = 12;

type Stage =
  | { kind: "preparing" }
  | { kind: "error"; message: string }
  | { kind: "ready"; preview: ForkPreview }
  | { kind: "running"; preview: ForkPreview; phase: ForkPhase | null; cancelling: boolean }
  | { kind: "done"; preview: ForkPreview; outcome: ForkOutcome };

/** Fork a feature into a new worktree. Nothing is created until Fork feature
 * is pressed, and the confirmation is bound to the exact source, session and
 * HEAD in the preview: if any of them moved, the fork is refused rather than
 * retargeted. */
export default function ForkFeatureDialog({ target, sessionId, supported, onClose, onOpenFork }: {
  target: FeatureTarget;
  sessionId: string | null;
  supported: HarnessInfo[];
  onClose: () => void;
  onOpenFork: (target: FeatureTarget) => void;
}) {
  const [stage, setStage] = useState<Stage>({ kind: "preparing" });
  const [attempt, setAttempt] = useState(0);
  const [branch, setBranch] = useState("");
  const [agent, setAgent] = useState<AgentSlug | null>(null);
  const [includeContext, setIncludeContext] = useState(false);
  const [hookChoice, setHookChoice] = useState("");
  const [startAgent, setStartAgent] = useState(true);
  const [approval, setApproval] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const alive = useRef(true);
  const token = stage.kind === "preparing" || stage.kind === "error" ? null : stage.preview.token;

  // A stale approval prompt must not survive an edit to the request it was for.
  useEffect(() => {
    setApproval(null);
  }, [branch, agent, includeContext, hookChoice, startAgent]);

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  useEffect(() => {
    let live = true;
    setStage({ kind: "preparing" });
    forkPrepare(target, sessionId).then((preview) => {
      if (!live) {
        // Closed while reading: release the server-side preview.
        void forkCancel(preview.token).catch(() => undefined);
        return;
      }
      setBranch(preview.suggested_branch);
      setAgent(preview.default_agent);
      setIncludeContext(preview.context.status === "available");
      setHookChoice("");
      setStage({ kind: "ready", preview });
    }).catch((err) => {
      if (live) setStage({ kind: "error", message: asGuiError(err).message });
    });
    return () => { live = false; };
  }, [target.project_id, target.feature_id, sessionId, attempt]);

  const choices = useQuery({
    queryKey: ["project-harness-choices", target.project_id],
    queryFn: () => projectHarnessChoices(target.project_id),
    retry: false, staleTime: 0, refetchOnWindowFocus: false,
  });
  const offered = choices.data?.agents ?? [];
  const harnessName = (slug: AgentSlug) => supported.find((info) => info.slug === slug)?.display_name ?? slug;

  // Report the running phase; the confirm call itself only returns at the end.
  const running = stage.kind === "running";
  useEffect(() => {
    if (!running || !token) return;
    const timer = setInterval(() => {
      forkStatus(token).then((status) => {
        if (!alive.current) return;
        setStage((current) => current.kind === "running"
          ? { ...current, phase: status.phase, cancelling: status.cancel_requested } : current);
      }).catch(() => undefined);
    }, 400);
    return () => clearInterval(timer);
  }, [running, token]);

  function confirm(approveResources: boolean) {
    if (stage.kind !== "ready" || !agent) return;
    const { preview } = stage;
    setProblem(null);
    setApproval(null);
    setStage({ kind: "running", preview, phase: null, cancelling: false });
    forkConfirm({
      token: preview.token,
      expected: {
        project_id: preview.source.project_id,
        feature_id: preview.source.feature_id,
        session_id: preview.source.session?.id ?? null,
        branch: preview.source.branch,
        head: preview.source.head,
      },
      new_branch: branch.trim(),
      agent,
      include_context: includeContext,
      hook_choice: preview.hook ? hookChoice : null,
      start_agent: startAgent,
      approve_resources: approveResources,
    }).then((outcome) => {
      if (alive.current) setStage({ kind: "done", preview, outcome });
    }).catch((err) => {
      if (!alive.current) return;
      const error = asGuiError(err);
      // A refusal leaves the preview usable unless the backend dropped its
      // token (the source moved): then only preparing again can proceed.
      forkStatus(preview.token).then(() => {
        if (!alive.current) return;
        if (error.kind === "needs_approval") setApproval(error.message);
        else setProblem(error.message);
        setStage({ kind: "ready", preview });
      }, () => {
        if (alive.current) setStage({ kind: "error", message: error.message });
      });
    });
  }

  function close() {
    if (stage.kind === "running") return;
    if (stage.kind === "ready") void forkCancel(stage.preview.token).catch(() => undefined);
    onClose();
  }

  const preview = stage.kind === "ready" || stage.kind === "running" || stage.kind === "done" ? stage.preview : null;
  const hookMissing = !!preview?.hook && !hookChoice;
  const agentUnavailable = choices.isSuccess && agent !== null && !offered.includes(agent);
  const canFork = stage.kind === "ready" && !!branch.trim() && !!agent && !hookMissing
    && !agentUnavailable && choices.isSuccess;

  return (
    <Modal
      label="Fork feature"
      title="Fork feature"
      subtitle={preview ? `From ${preview.source.project_name} / ${preview.source.feature_name}` : undefined}
      size="md"
      onClose={close}
      dismissable={stage.kind !== "running"}
      onSubmit={() => { if (canFork) confirm(false); }}
      footer={
        stage.kind === "done" ? (
          <>
            {stage.outcome.status === "created" && (
              <button type="button" className="btn btn-primary"
                onClick={() => { onOpenFork((stage.outcome as Extract<ForkOutcome, { status: "created" }>).feature); onClose(); }}>
                Open fork
              </button>
            )}
            <button type="button" className="btn btn-ghost" onClick={onClose}>Close</button>
          </>
        ) : stage.kind === "running" ? (
          <button type="button" className="btn btn-ghost" disabled={stage.cancelling}
            onClick={() => { setStage({ ...stage, cancelling: true }); void forkCancel(stage.preview.token).catch(() => undefined); }}>
            {stage.cancelling ? "Cancelling…" : "Cancel fork"}
          </button>
        ) : (
          <>
            <button type="button" className="btn btn-ghost" onClick={close}>Cancel</button>
            {stage.kind === "error" && (
              <button type="button" className="btn btn-secondary" onClick={() => setAttempt((n) => n + 1)}>Try again</button>
            )}
            {approval ? (
              <button type="button" className="btn btn-warning" disabled={!canFork} onClick={() => confirm(true)}>Approve and fork</button>
            ) : stage.kind !== "error" && (
              <button type="submit" className="btn btn-primary" disabled={!canFork}>Fork feature</button>
            )}
          </>
        )
      }
    >
      {stage.kind === "preparing" && <p role="status"><Spinner /> Reading the source checkout and its working changes…</p>}
      {stage.kind === "error" && (
        <div className="callout callout-danger" role="alert"><Icon name="alert" /><p>{stage.message}</p></div>
      )}
      {preview && stage.kind !== "done" && (
        <fieldset className="fork-form" disabled={stage.kind === "running"}>
          <p className="muted fork-source">
            <Icon name="branch" /> <strong>{preview.source.branch}</strong> at <code>{preview.source.head.slice(0, 10)}</code>
            {preview.source.session && <> · session <strong>{preview.source.session.label}</strong></>}
          </p>
          <section aria-label="Working changes" className="fork-changes">
            <h4>
              {preview.changes.total === 0
                ? "No uncommitted changes to copy"
                : `${preview.changes.total} uncommitted ${preview.changes.total === 1 ? "change" : "changes"} will be copied`}
            </h4>
            {preview.changes.total > 0 && (
              <ul>
                {preview.changes.listed.slice(0, SHOWN_CHANGES).map((change) => (
                  <li key={change.path}><span className={`badge fork-kind fork-kind-${change.kind}`}>{KIND_LABEL[change.kind]}</span> <code>{change.path}</code></li>
                ))}
                {preview.changes.total > SHOWN_CHANGES && (
                  <li className="muted">
                    and {preview.changes.total - SHOWN_CHANGES} more
                    {preview.changes.truncated ? " (the list is shortened)" : ""}
                  </li>
                )}
              </ul>
            )}
          </section>
          <div className="callout callout-info"><Icon name="check" /><p>{preview.staging_note}</p></div>
          <Field label="New branch / feature name">
            <input value={branch} onChange={(event) => setBranch(event.target.value)} spellCheck={false} />
          </Field>
          <Field label="Harness" hint={choices.isError ? `Could not load project harnesses: ${asGuiError(choices.error).message}` : `Mode ${preview.mode} is inherited from the source.`}>
            <select value={agent ?? ""} disabled={!choices.isSuccess} onChange={(event) => setAgent(event.target.value as AgentSlug)}>
              {agent && !offered.includes(agent) && <option value={agent} disabled>{harnessName(agent)} — unavailable</option>}
              {offered.map((slug) => <option key={slug} value={slug}>{harnessName(slug)}</option>)}
            </select>
          </Field>
          {agentUnavailable && (
            <p role="alert" className="callout callout-warning">{harnessName(agent!)} is not available for this project. Choose another harness.</p>
          )}
          <Switch
            checked={includeContext}
            disabled={preview.context.status !== "available"}
            onChange={setIncludeContext}
            label="Include the conversation as .claude/context.md"
            hint={preview.context.message}
          />
          {preview.hook && (
            <Field label={preview.hook.title} hint="The project's worktree hook runs in the new checkout after it is registered.">
              <select value={hookChoice} onChange={(event) => setHookChoice(event.target.value)}>
                <option value="" disabled>Choose…</option>
                {preview.hook.options.map((option) => <option key={option} value={option}>{option}</option>)}
              </select>
            </Field>
          )}
          <Switch
            checked={startAgent}
            onChange={setStartAgent}
            label="Start the agent after forking"
            hint={startAgent ? "Starting counts against AMF's agent and memory limits." : "The fork is created stopped; start it from the sidebar."}
          />
        </fieldset>
      )}
      {approval && (
        <div className="callout callout-warning" role="alert"><Icon name="alert" /><p>{approval}</p></div>
      )}
      {problem && (
        <div className="callout callout-danger" role="alert"><Icon name="alert" /><p>{problem}</p></div>
      )}
      {stage.kind === "running" && (
        <p role="status"><Spinner /> {stage.phase ? PHASE_LABEL[stage.phase] : "Starting…"}</p>
      )}
      {stage.kind === "done" && <ForkResult outcome={stage.outcome} />}
    </Modal>
  );
}

function ForkResult({ outcome }: { outcome: ForkOutcome }) {
  if (outcome.status === "created") {
    return (
      <div role="status" className="fork-result">
        <div className={`callout ${outcome.notes.length ? "callout-warning" : "callout-info"}`}>
          <Icon name="check" /><p>{outcome.message}</p>
        </div>
        <dl className="fork-facts">
          <dt>Worktree</dt><dd><code>{outcome.workdir}</code></dd>
          <dt>Copied</dt><dd>{outcome.transferred} working {outcome.transferred === 1 ? "change" : "changes"}{outcome.context_written ? " and the conversation" : ""}</dd>
          <dt>Agent</dt><dd>{outcome.started ? "Started" : "Not started (the feature is stopped)"}</dd>
          {outcome.hook && <><dt>Hook</dt><dd>{outcome.hook.cancelled ? "Cancelled" : outcome.hook.succeeded ? "Succeeded" : `Failed${outcome.hook.detail ? `: ${outcome.hook.detail}` : ""}`}</dd></>}
        </dl>
        {outcome.notes.map((note) => <p key={note} className="muted">{note}</p>)}
      </div>
    );
  }
  if (outcome.status === "cancelled") {
    return <div role="status" className="callout callout-info"><Icon name="check" /><p>{outcome.message}. No worktree was created.</p></div>;
  }
  return (
    <div role="alert" className="fork-result">
      <div className="callout callout-danger"><Icon name="alert" /><p>{outcome.message}</p></div>
      <dl className="fork-facts">
        <dt>Stopped at</dt><dd>{PHASE_LABEL[outcome.phase].replace("…", "")}</dd>
        {outcome.retained
          ? <><dt>Kept checkout</dt><dd><code>{outcome.retained.workdir}</code> on branch <code>{outcome.retained.branch}</code></dd></>
          : <><dt>Checkout</dt><dd>None remains.</dd></>}
        <dt>Agent</dt><dd>Not started</dd>
      </dl>
    </div>
  );
}
