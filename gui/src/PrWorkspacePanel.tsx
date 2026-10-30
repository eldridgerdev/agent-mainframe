import { useCallback, useEffect, useRef, useState } from "react";
import { asGuiError, type FeatureTarget } from "./api";
import { Hunk } from "./DiffPanel";
import Markdown from "./Markdown";
import { Spinner } from "./ui";
import PrTriagePanel from "./PrTriagePanel";
import type { PrFixHandoff } from "./prTriageApi";
import { prTriageAct, prTriageBegin, type PrTriageView, type PrFixTarget } from "./prTriageApi";
import { workspaceAiAct, workspaceAiSnapshot, workspaceBegin, workspaceClose, workspaceFiles, workspaceSelectPr, workspaceSwitchAi, workspaceSwitchThreads, workspacePrepareFix, workspaceFixTargets, type WorkspaceAi, type WorkspaceFiles } from "./prWorkspaceApi";

type Tab = "files" | "threads" | "ai";

export type PrWorkspaceStatus = { running: boolean; prNumber: number | null; completed: boolean };

function elapsedLabel(seconds: number) {
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return minutes ? `${minutes}m ${remainder}s` : `${remainder}s`;
}

/** Experimental three-pane PR workspace. The legacy PR Triage modal remains a separate route. */
export default function PrWorkspacePanel({ target, onClose, onReturnToFeature, onStatusChange, onHandoff }: { target: FeatureTarget; onClose: () => void; onReturnToFeature: () => void; onStatusChange: (status: PrWorkspaceStatus) => void; onHandoff?: (handoff: PrFixHandoff) => void }) {
  const [workflowId, setWorkflowId] = useState<string | null>(null);
  const [triage, setTriage] = useState<PrTriageView | null>(null);
  const [files, setFiles] = useState<WorkspaceFiles | null>(null);
  const [ai, setAi] = useState<WorkspaceAi | null>(null);
  const [tab, setTab] = useState<Tab>("threads");
  const [path, setPath] = useState<string | null>(null);
  const [selectedFindings, setSelectedFindings] = useState<Set<number>>(new Set());
  const [fixTarget, setFixTarget] = useState<PrFixTarget | null>(null);
  const [fixTargets, setFixTargets] = useState<PrFixTarget[]>([]);
  const handoffPending = useRef(false);
  const handoffGeneration = useRef(0);
  const handoffContext = useRef("");
  handoffContext.current = JSON.stringify([workflowId, ai?.result_revision, triage?.review?.number, triage?.review?.head_sha, fixTarget, fixTargets, [...selectedFindings].sort((a, b) => a - b)]);
  const [publishConfirm, setPublishConfirm] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [busy, setBusy] = useState(true);
  const [triageBusy, setTriageBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const aiLocalStartedAt = useRef<number | null>(null);
  const [clockNow, setClockNow] = useState(() => Date.now());
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
        if (live) setError(asGuiError(e).message);
      } finally { if (live) setBusy(false); }
    })();
    return () => {
      live = false;
      handoffGeneration.current++;
      queueMicrotask(release);
    };
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
        setFixTargets(await workspaceFixTargets(workflowId));
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
    catch (e) { setError(asGuiError(e).message); }
    finally { setBusy(false); }
  };

  const publishableFindings = ai?.findings.filter((finding) => !finding.skipped && !finding.published) ?? [];
  const fixableFindingIndices = ai?.findings.flatMap((finding, index) => !finding.skipped && !finding.published ? [index] : []) ?? [];
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
    const chosen = fixTargets.find((item) => item.target.session_id === fixTarget.target.session_id && item.destination_revision === fixTarget.destination_revision && !item.stopped);
    if (!chosen) { setError("The selected agent session is unavailable or changed; choose a target again."); return; }
    const generation = ++handoffGeneration.current;
    const context = handoffContext.current;
    handoffPending.current = true;
    setBusy(true);
    try {
      const handoff = await workspacePrepareFix({ workflow_id: workflowId, result_revision: ai.result_revision, finding_indices: indices, target: fixTarget.target, destination_revision: fixTarget.destination_revision });
      if (handoffGeneration.current === generation && handoffContext.current === context) { setError(null); onHandoff(handoff); }
    } catch (e) {
      if (handoffGeneration.current === generation && handoffContext.current === context) {
        setError(asGuiError(e).message);
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

  const requestClose = useCallback(async () => {
    if (busy) return;
    try {
      if (workflowId && tab === "ai") {
        await workspaceSwitchThreads(workflowId);
        setTab("threads");
      }
    } catch (e) {
      setError(asGuiError(e).message);
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
    const needsThreadRestore = changedPr && tabRef.current === "ai";
    if (identity !== prIdentity.current) {
      if (changedPr) {
        setFiles(null);
        setAi(null);
        setPath(null);
        setSelectedFindings(new Set());
        setPublishConfirm(false);
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
      } catch (e) { setError(asGuiError(e).message); }
      finally { if (needsThreadRestore) setBusy(false); }
    })();
  }, [workflowId, triage?.review?.number, triage?.review?.head_sha]);

  const review = triage?.review;
  const file = files?.files.find((f) => f.path === path) ?? files?.files[0] ?? null;
  const aiElapsedSeconds = ai?.running && aiLocalStartedAt.current !== null
    ? Math.max(ai.elapsed_seconds, Math.floor((clockNow - aiLocalStartedAt.current) / 1000))
    : ai?.elapsed_seconds ?? 0;

  return <section className="pr-workspace-page" aria-label="Experimental PR Workspace">
    <header className="pr-workspace-page-header">
      <div className="pr-workspace-page-kicker"><span>Feature workspace</span><span className="tag tag-accent">Experimental</span></div>
      <div className="pr-workspace-page-title-row">
        <div><h1>Pull request workspace</h1><p className="muted">{triage?.feature_name ?? "Feature review"} · files, review threads and AI findings in one place</p></div>
        <div className="pr-workspace-page-actions">
          {ai?.running && <span className="pr-workspace-running-label"><Spinner /> Reviewing PR #{ai.pr_number}</span>}
          <button type="button" className="btn btn-secondary" onClick={() => { if (handoffPending.current) { handoffGeneration.current++; setBusy(false); } onReturnToFeature(); }}>Back to feature</button>
        </div>
      </div>
    </header>
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
          <PrTriagePanel target={target} embedded embeddedActive={tab === "threads"} embeddedView={triage} onEmbeddedViewChange={setTriage} onEmbeddedBusyChange={setTriageBusy} onRequestClose={(close) => { triageClose.current = close; }} onClose={() => void closeWorkspace(true)} onHandoff={onHandoff} />
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
            : <div className="pr-ai-results">
              <header className="pr-ai-results-header"><div><span className="eyebrow">AI review</span><h2>PR #{ai?.pr_number ?? review.number}</h2><p className="muted">Head <code>{ai?.head_sha.slice(0, 7) ?? review.head_sha.slice(0, 7)}</code></p></div><div className="pr-ai-results-actions">{ai?.findings.length ? <span className="tag tag-accent">{ai.findings.length} {ai.findings.length === 1 ? "finding" : "findings"}</span> : ai?.summary && !ai.error ? <span className="tag tag-success">No findings</span> : null}{publishableFindings.length > 0 && <button className="btn btn-secondary" disabled={busy || ai?.running} onClick={() => setPublishConfirm(true)}>Post to GitHub…</button>}{ai && ai.pr_number !== review.number && <button className="btn btn-secondary" disabled={busy} onClick={() => void switchTab("ai")}>Switch to selected PR #{review.number}</button>}<button className="btn btn-primary" disabled={busy} onClick={() => void aiAct("start")}>{ai?.findings.length || ai?.summary ? "Run again" : "Start AI review"}</button></div></header>
              {publishConfirm && <section className="pr-ai-publish-confirm" role="alertdialog" aria-label="Confirm post AI review to GitHub"><div><span className="eyebrow">GitHub review</span><h3>Post {publishableFindings.length} {publishableFindings.length === 1 ? "finding" : "findings"} to PR #{ai?.pr_number}?</h3><p>This creates a comment review on GitHub as your <code>gh</code> account. The findings below will be visible to PR reviewers.</p><ul>{publishableFindings.map((finding, index) => <li key={`${finding.path}:${finding.line}:${index}`}><code>{finding.path ? `${finding.path}${finding.line ? `:${finding.line}` : ""}` : "General finding"}</code> — {finding.body.split("\n")[0]}</li>)}</ul></div><div className="pr-ai-results-actions"><button className="btn btn-primary" disabled={busy} onClick={() => void aiAct("post")}>{busy ? "Posting review…" : "Post review to GitHub"}</button><button className="btn btn-secondary" disabled={busy} onClick={() => setPublishConfirm(false)}>Cancel</button></div></section>}
              {ai?.error && <p className="callout callout-danger" role="alert">{ai.error}</p>}
              {ai?.summary && <section className="pr-ai-summary" aria-label="Review summary"><div className="pr-ai-summary-title"><span className="pr-ai-summary-mark">✓</span><div><span className="eyebrow">Review complete</span><h3>Summary</h3></div></div><Markdown source={ai.summary} /></section>}
              {!!ai?.findings.length && <section className="pr-ai-findings-section" aria-label="Review findings"><div className="pr-ai-findings-heading"><h3>Findings</h3><span className="muted small">Ordered by appearance in the diff</span></div>{!!onHandoff && <div className="pr-ai-batch-toolbar"><label>Fix agent<select value={fixTarget?.target.session_id ?? ""} disabled={busy} onChange={(event) => setFixTarget(fixTargets.find((item) => item.target.session_id === event.target.value && !item.stopped) ?? null)}><option value="">Choose an agent session</option>{fixTarget && !fixTargets.some((item) => item.target.session_id === fixTarget.target.session_id && item.destination_revision === fixTarget.destination_revision && !item.stopped) && <option value={fixTarget.target.session_id} disabled>Previously selected session unavailable or changed</option>}{fixTargets.filter((item) => !item.stopped && (item.target.session_id !== fixTarget?.target.session_id || item.destination_revision === fixTarget.destination_revision)).map((item) => <option key={item.target.session_id} value={item.target.session_id}>{item.label}</option>)}</select></label><button className="btn btn-ghost btn-sm" disabled={busy} onClick={() => { if (workflowId) void workspaceFixTargets(workflowId).then(setFixTargets).catch((e) => setError(asGuiError(e).message)); }}>Refresh agents</button><div className="pr-ai-batch-actions"><button className="btn btn-ghost btn-sm" disabled={busy || !fixableFindingIndices.length} onClick={toggleAllFixableFindings}>{allFixableSelected ? "Clear selection" : "Select all"}</button><button className="btn btn-secondary" disabled={busy || !onHandoff || !fixTarget || !selectedFindings.size} onClick={() => prepareFix([...selectedFindings].sort((a, b) => a - b))}>Prepare batch fix ({selectedFindings.size})</button></div></div>}<ol className="pr-ai-findings">{ai.findings.map((finding, i) => <li className="pr-ai-finding" key={`${finding.path}:${finding.line}:${i}`}><span className="pr-ai-finding-index">{String(i + 1).padStart(2, "0")}</span><article><header><code>{finding.path || "General"}{finding.line ? `:${finding.line}` : ""}</code><div className="pr-ai-finding-tools">{finding.published ? <span className="tag tag-success">Posted</span> : finding.skipped ? <span className="tag">Skipped</span> : <><span className="tag">Review finding</span>{onHandoff && fixTargets.some((item) => !item.stopped) && <><label className="pr-ai-select-finding"><input type="checkbox" checked={selectedFindings.has(i)} disabled={busy} onChange={(event) => setSelectedFindings((current) => { const next = new Set(current); if (event.target.checked) next.add(i); else next.delete(i); return next; })} />Batch</label><button className="btn btn-ghost btn-sm" disabled={busy || !fixTarget} onClick={() => prepareFix([i])}>Prepare fix</button></>}</>}</div></header><Markdown source={finding.body} />{finding.diff_hunk && <details className="pr-ai-finding-diff"><summary>Show related diff</summary><pre className="pr-snippet">{finding.diff_hunk}</pre></details>}</article></li>)}</ol></section>}
              {ai && !ai.findings.length && !ai.error && !ai.summary && <div className="pr-ai-empty"><span className="pr-ai-summary-mark">✦</span><h3>Ready to review</h3><p className="muted">Run an AI review to inspect the selected PR for correctness issues.</p></div>}
            </div>}
        </section>}
      </section>
    </div>
    <footer className="pr-workspace-footer"><span className="muted small">Files and AI findings are pinned to the selected PR head.</span><button className="btn btn-ghost btn-sm" disabled={busy || triageBusy || ai?.running || !!ai?.precall} onClick={requestClose}>Close workspace</button></footer>
  </section>;
}
