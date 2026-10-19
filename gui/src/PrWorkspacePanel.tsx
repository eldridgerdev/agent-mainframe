import { useCallback, useEffect, useRef, useState } from "react";
import { asGuiError, type FeatureTarget } from "./api";
import { Hunk } from "./DiffPanel";
import Markdown from "./Markdown";
import { Modal, Spinner } from "./ui";
import PrTriagePanel from "./PrTriagePanel";
import type { PrFixHandoff } from "./prTriageApi";
import { prTriageAct, prTriageBegin, type PrTriageView } from "./prTriageApi";
import { workspaceAiAct, workspaceAiSnapshot, workspaceBegin, workspaceClose, workspaceFiles, workspaceSelectPr, workspaceSwitchAi, workspaceSwitchThreads, type WorkspaceAi, type WorkspaceFiles } from "./prWorkspaceApi";

type Tab = "files" | "threads" | "ai";

function elapsedLabel(seconds: number) {
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return minutes ? `${minutes}m ${remainder}s` : `${remainder}s`;
}

/** Experimental three-pane PR workspace. The legacy PR Triage modal remains a separate route. */
export default function PrWorkspacePanel({ target, onClose, onHandoff }: { target: FeatureTarget; onClose: () => void; onHandoff?: (handoff: PrFixHandoff) => void }) {
  const [workflowId, setWorkflowId] = useState<string | null>(null);
  const [triage, setTriage] = useState<PrTriageView | null>(null);
  const [files, setFiles] = useState<WorkspaceFiles | null>(null);
  const [ai, setAi] = useState<WorkspaceAi | null>(null);
  const [tab, setTab] = useState<Tab>("threads");
  const [path, setPath] = useState<string | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [busy, setBusy] = useState(true);
  const [triageBusy, setTriageBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const aiLocalStartedAt = useRef<number | null>(null);
  const [clockNow, setClockNow] = useState(() => Date.now());
  const prIdentity = useRef<string | null>(null);
  const triageClose = useRef<(() => void) | null>(null);

  useEffect(() => {
    let live = true;
    let id: string | null = null;
    void (async () => {
      try {
        id = await workspaceBegin(target);
        const view = await prTriageBegin(target);
        if (!live) return;
        setWorkflowId(id); setTriage(view);
      } catch (e) {
        if (id) void workspaceClose(id).catch(() => undefined);
        if (live) setError(asGuiError(e).message);
      } finally { if (live) setBusy(false); }
    })();
    return () => { live = false; };
  }, [target.project_id, target.feature_id]);

  const loadFiles = useCallback(async (id: string) => {
    setBusy(true);
    try { const result = await workspaceFiles(id); setFiles(result); setPath((p) => result.files.some((f) => f.path === p) ? p : result.files[0]?.path ?? null); setError(null); }
    catch (e) { setError(asGuiError(e).message); }
    finally { setBusy(false); }
  }, []);

  const switchTab = useCallback(async (next: Tab) => {
    if (!workflowId || busy || triageBusy) return;
    setBusy(true); setError(null);
    try {
      if (next === "ai") {
        const number = triage?.review?.number;
        if (number == null) throw new Error("Open a pull request in Threads first.");
        await workspaceSelectPr(workflowId, number);
        await workspaceSwitchAi(workflowId, number);
        setAi(await workspaceAiSnapshot(workflowId));
      } else {
        await workspaceSwitchThreads(workflowId);
        if (next === "files") await loadFiles(workflowId);
      }
      setTab(next);
    } catch (e) { setError(asGuiError(e).message); }
    finally { setBusy(false); }
  }, [workflowId, busy, triageBusy, triage, loadFiles]);

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
        if (live) setError(asGuiError(e).message);
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

  const aiAct = async (kind: "start" | "cancel" | "confirm_precall" | "cancel_precall") => {
    if (!workflowId || busy) return;
    setBusy(true);
    try { setAi(await workspaceAiAct(workflowId, { kind })); setError(null); }
    catch (e) { setError(asGuiError(e).message); }
    finally { setBusy(false); }
  };

  const closeWorkspace = useCallback(async (triageAlreadyClosed = false) => {
    if (!workflowId) { onClose(); return; }
    if (busy) return;
    setBusy(true);
    try {
      if (tab === "ai") await workspaceSwitchThreads(workflowId);
      if (triage && !triageAlreadyClosed) await prTriageAct(triage, { kind: "close" });
      await workspaceClose(workflowId);
      onClose();
    } catch (e) { setError(asGuiError(e).message); }
    finally { setBusy(false); }
  }, [workflowId, busy, tab, triage, onClose]);

  const requestClose = useCallback(() => {
    if (triageClose.current) triageClose.current();
    else void closeWorkspace();
  }, [closeWorkspace]);

  useEffect(() => {
    if (!workflowId || !triage) return;
    const number = triage.review?.number ?? 0;
    const identity = triage.review ? `${triage.review.number}:${triage.review.head_sha}` : null;
    if (identity !== prIdentity.current) {
      if (prIdentity.current !== null) {
        setFiles(null);
        setAi(null);
        setPath(null);
        setTab("threads");
      }
      prIdentity.current = identity;
    }
    void workspaceSelectPr(workflowId, number).catch((e) => setError(asGuiError(e).message));
  }, [workflowId, triage?.review?.number, triage?.review?.head_sha]);

  const review = triage?.review;
  const file = files?.files.find((f) => f.path === path) ?? files?.files[0] ?? null;
  const aiElapsedSeconds = ai?.running && aiLocalStartedAt.current !== null
    ? Math.max(ai.elapsed_seconds, Math.floor((clockNow - aiLocalStartedAt.current) / 1000))
    : ai?.elapsed_seconds ?? 0;

  return <Modal label="Experimental PR Workspace" title={`PR Workspace${triage ? ` · ${triage.feature_name}` : ""}`} size="workspace" dismissable={!busy && !triageBusy && !ai?.running && !ai?.precall} onClose={requestClose}>
    <p className="muted small">Experimental · shared PR files, review threads and AI review</p>
    {(error || triage?.error) && <p className="callout callout-danger" role="alert">{error || triage?.error}</p>}
    <div className={`pr-workspace-layout${sidebarOpen ? "" : " pr-workspace-layout-collapsed"}`}>
      <aside className={`pr-workspace-context${sidebarOpen ? "" : " pr-workspace-context-collapsed"}`} aria-label="Pull request context">
        <div className="pr-workspace-context-heading">
          {sidebarOpen && <strong>{review ? `PR #${review.number}` : "Pull requests"}</strong>}
          <button type="button" className="btn btn-icon btn-ghost btn-sm" aria-label={sidebarOpen ? "Collapse PR sidebar" : "Expand PR sidebar"} aria-expanded={sidebarOpen} title={sidebarOpen ? "Collapse PR sidebar" : "Expand PR sidebar"} onClick={() => setSidebarOpen((open) => !open)}>{sidebarOpen ? "‹" : "›"}</button>
        </div>
        {sidebarOpen && review && <><p className="small muted">{review.head_ref} · {review.head_sha.slice(0, 7)}</p><p className="small">{review.open_count} open of {review.total} review items</p><p className="small muted">Fetched {review.fetched_at}</p><button className="btn btn-secondary btn-sm" disabled={busy || triageBusy} onClick={() => void switchTab("threads")}>Review threads</button></>}
        {sidebarOpen && !review && <p className="muted small">Choose a PR in Threads to load its files and AI review.</p>}
        {!sidebarOpen && <span className="pr-workspace-context-badge" title={review ? `PR #${review.number}` : "Pull requests"}>{review ? `#${review.number}` : "PR"}</span>}
      </aside>
      <section className="pr-workspace-main">
        <nav className="pr-workspace-tabs" aria-label="PR workspace views">
          {(["files", "threads", "ai"] as const).map((item) => <button key={item} className={`btn ${tab === item ? "btn-primary" : "btn-secondary"}`} aria-current={tab === item ? "page" : undefined} disabled={busy || triageBusy || !review && item !== "threads"} onClick={() => void switchTab(item)}>{item === "ai" ? `AI Review${ai?.running ? " · Running" : ""}` : item[0].toUpperCase() + item.slice(1)}</button>)}
        </nav>
        {!review && tab === "threads" && <p className="muted">Choose the highlighted PR to load its threads and workspace.</p>}
        {review && tab === "files" && <div className="pr-reader">
          <nav className="diff-files" aria-label="PR changed files">{files?.files.map((f) => <button key={f.path} className={`diff-file ${file === f ? "diff-file-selected" : ""}`} onClick={() => setPath(f.path)}>{f.path}<small>+{f.additions} −{f.deletions}</small></button>)}</nav>
          <section className="diff-content" aria-label="PR file diff">{file ? <><strong>{file.path}</strong><div className="diff-code">{file.is_binary ? <p>Binary file changed.</p> : file.hunks.length ? file.hunks.map((hunk, i) => <Hunk key={i} hunk={hunk} split={false} />) : <pre>{file.patch || "No textual changes."}</pre>}</div></> : <p className="muted">{busy ? <><Spinner /> Loading PR files…</> : "No changed files."}</p>}</section>
        </div>}
        {workflowId && <div className="pr-workspace-threads" hidden={tab !== "threads"}>
          <PrTriagePanel target={target} embedded embeddedView={triage} onEmbeddedViewChange={setTriage} onEmbeddedBusyChange={setTriageBusy} onRequestClose={(close) => { triageClose.current = close; }} onClose={() => void closeWorkspace(true)} onHandoff={onHandoff} />
        </div>}
        {review && tab === "ai" && <section className="pr-workspace-ai" aria-label="AI PR review">
          {ai?.precall ? <div className="callout"><h3>{ai.precall.title}</h3><p>{ai.precall.harness}</p><details><summary>Review prompt</summary><pre>{ai.precall.preview}</pre></details><button className="btn btn-secondary" disabled={busy} onClick={() => void aiAct("cancel_precall")}>Cancel AI review</button><button className="btn btn-primary" disabled={busy} onClick={() => void aiAct("confirm_precall")}>Continue</button></div>
            : ai?.running ? <section className="pr-ai-progress" aria-label={`AI review running on PR #${ai.pr_number}`}>
              <header className="pr-ai-progress-heading"><Spinner /><div><span className="tag tag-accent">In progress</span><h3>Reviewing PR #{ai.pr_number}</h3><p className="muted">The review runs in the background. You can switch tabs while it works.</p></div></header>
              <ol className="pr-ai-steps" aria-label="Review progress">
                <li className={ai.stage === "Fetching PR diff" ? "pr-ai-step-current" : "pr-ai-step-done"}><span>1</span><div><strong>Fetch PR changes</strong><small>{ai.stage === "Fetching PR diff" ? "Loading the latest diff from GitHub" : "Diff loaded"}</small></div></li>
                <li className={ai.stage === "Reviewing code" ? "pr-ai-step-current" : ""}><span>2</span><div><strong>Analyze the code</strong><small aria-live="polite">{ai.stage === "Reviewing code" ? ai.activity || "Waiting for the review harness to report progress" : "Starts when the diff is ready"}</small></div></li>
                <li><span>3</span><div><strong>Show findings</strong><small>Findings and summary appear here when the review finishes</small></div></li>
              </ol>
              <div className="pr-ai-metrics"><span>Elapsed <strong>{elapsedLabel(aiElapsedSeconds)}</strong></span><span>PR head <code>{ai.head_sha.slice(0, 7)}</code></span>{ai.token_estimate != null && <span>Prompt size <strong>~{ai.token_estimate.toLocaleString()} tokens</strong></span>}</div>
              <div className="pr-ai-progress-actions"><p className="muted small">Large changes can take a few minutes. If progress stops, you can cancel the run.</p><button className="btn btn-warning" disabled={busy} onClick={() => void aiAct("cancel")}>Cancel AI review</button></div>
            </section>
              : <><div className="row"><h3>AI review of PR #{ai?.pr_number ?? review.number}</h3>{ai && ai.pr_number !== review.number && <button className="btn btn-secondary" disabled={busy} onClick={() => void switchTab("ai")}>Switch to selected PR #{review.number}</button>}<button className="btn btn-primary" disabled={busy} onClick={() => void aiAct("start")}>{ai?.findings.length ? "Run again" : "Start AI review"}</button></div>{ai?.error && <p className="callout callout-danger">{ai.error}</p>}{ai?.summary && <Markdown source={ai.summary} />}{ai?.findings.map((finding, i) => <article className="callout" key={i}><strong>{finding.path || "General"}{finding.line ? `:${finding.line}` : ""}</strong><Markdown source={finding.body} />{finding.diff_hunk && <pre className="pr-snippet">{finding.diff_hunk}</pre>}</article>)}{ai && !ai.findings.length && !ai.error && <p className="muted">Run AI review to inspect the PR diff for findings.</p>}</>}
        </section>}
      </section>
    </div>
    <div className="row pr-workspace-footer"><span className="muted small">Files and AI findings are pinned to the selected PR head.</span><button className="btn btn-secondary" disabled={busy || triageBusy || ai?.running || !!ai?.precall} onClick={requestClose}>Close workspace</button></div>
  </Modal>;
}
