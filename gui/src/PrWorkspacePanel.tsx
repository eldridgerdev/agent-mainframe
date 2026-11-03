import { useEffect, useRef, useState } from "react";
import { asGuiError, type FeatureTarget, type GuiError } from "./api";
import GithubProblemNotice from "./GithubProblemNotice";
import { Hunk } from "./DiffPanel";
import PrReviewPanel from "./PrReviewPanel";
import { ExperimentalBadge, Icon, Spinner } from "./ui";
import PrTriagePanel from "./PrTriagePanel";
import type { PrFixHandoff } from "./prTriageApi";
import { workspaceFiles, type WorkspaceFiles } from "./prWorkspaceApi";
import { usePrWorkspace, type AiLifecycle, type Tab } from "./pr-workspace/usePrWorkspace";
import { useAiReview } from "./pr-workspace/useAiReview";
import AiReviewTab from "./pr-workspace/AiReviewTab";

export type PrWorkspaceStatus = { running: boolean; prNumber: number | null; completed: boolean };

/** The pre-embed Files tab: a read-only `gh pr diff`, shown only when the PR cannot be reviewed from Files. */
function ReadOnlyFiles({ workflowId }: { workflowId: string }) {
  const [files, setFiles] = useState<WorkspaceFiles | null>(null);
  const [path, setPath] = useState<string | null>(null);
  const [failure, setFailure] = useState<GuiError | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let live = true;
    workspaceFiles(workflowId).then((result) => { if (live) { setFailure(null); setFiles(result); setPath(result.files[0]?.path ?? null); } })
      .catch((e) => { if (live) setFailure(asGuiError(e)); });
    return () => { live = false; };
  }, [workflowId, attempt]);
  const file = files?.files.find((f) => f.path === path) ?? files?.files[0] ?? null;
  return <div className="pr-reader" aria-label="Read-only PR files">
    {failure?.github ? <GithubProblemNotice problem={failure.github} onRetry={() => setAttempt((n) => n + 1)} />
      : failure && <p className="callout callout-danger" role="alert">{failure.message}</p>}
    <nav className="diff-files" aria-label="PR changed files">{files?.files.map((f) => <button key={f.path} className={`diff-file ${file === f ? "diff-file-selected" : ""}`} onClick={() => setPath(f.path)}>{f.path}<small>+{f.additions} −{f.deletions}</small></button>)}</nav>
    <section className="diff-content" aria-label="PR file diff">{file ? <><strong>{file.path}</strong><div className="diff-code">{file.is_binary ? <p>Binary file changed.</p> : file.hunks.length ? file.hunks.map((hunk, i) => <Hunk key={i} hunk={hunk} split={false} />) : <pre>{file.patch || "No textual changes."}</pre>}</div></> : !failure && <p className="muted"><Spinner /> Loading PR files…</p>}</section>
  </div>;
}

/** Experimental three-pane PR workspace. The legacy PR Triage modal remains a separate route. */
export default function PrWorkspacePanel({ target, onClose, onReturnToFeature, onStatusChange, onHandoff, onOpenPrReview }: { target: FeatureTarget; onClose: () => void; onReturnToFeature: () => void; onStatusChange: (status: PrWorkspaceStatus) => void; onHandoff?: (handoff: PrFixHandoff) => void; onOpenPrReview?: (projectId: string) => void }) {
  const aiLifecycle = useRef<AiLifecycle | null>(null);
  const workspace = usePrWorkspace({ target, onClose, onOpenPrReview, ai: aiLifecycle });
  const { workflowId, triage, setTriage, filesDirty, setFilesDirty, tab, busy, setBusy, triageBusy, setTriageBusy, failure, setFailure, error, setAttempt, handoffPending, handoffGeneration, triageClose, switchTab, closeWorkspace, requestClose } = workspace;
  const aiReview = useAiReview({ workspace, onHandoff, onStatusChange });
  aiLifecycle.current = aiReview.lifecycle;
  const { ai } = aiReview;
  const [sidebarOpen, setSidebarOpen] = useState(true);

  const review = triage?.review;
  // Why a tab can't be chosen right now, shown as its tooltip; null when it can.
  const tabLockReason = (item: Tab): string | null =>
    !review && item !== "threads" ? "Open a pull request in Threads first."
      : filesDirty && tab === "files" ? "Save or discard the unsaved comment first."
      : busy || triageBusy ? "Wait for the current action to finish."
      : null;
  // Threads shows the triage view's own GitHub problem; other tabs show it here.
  const githubProblem = failure?.github ?? (tab !== "threads" ? triage?.github_problem ?? null : null);

  return <section className="pr-workspace-page" aria-label="PR workspace">
    <header className="pr-workspace-page-header">
      <div className="pr-workspace-page-kicker"><span>Feature workspace</span><ExperimentalBadge title="New and still changing. Your GitHub data is not affected." /></div>
      <div className="pr-workspace-page-title-row">
        <div><h1>PR workspace</h1><p className="muted">{triage?.feature_name ?? "Feature review"} · files, review threads and AI findings in one place. Review comments and Write a review do the same jobs one at a time.</p></div>
        <div className="pr-workspace-page-actions">
          {ai?.running && <span className="pr-workspace-running-label"><Spinner /> Reviewing PR #{ai.pr_number}</span>}
          <button type="button" className="btn btn-secondary" onClick={() => { if (handoffPending.current) { handoffGeneration.current++; setBusy(false); } onReturnToFeature(); }} title="Return to the feature. The workspace stays open in the background.">Back to feature</button>
        </div>
      </div>
    </header>
    {githubProblem && <GithubProblemNotice problem={githubProblem} retrying={busy}
      onRetry={!workflowId && !triage ? () => { setFailure(null); setBusy(true); setAttempt((n) => n + 1); } : undefined} />}
    {/* A local refusal is shown even beside the triage view's GitHub problem. */}
    {(error ? !failure?.github : triage?.error && !triage?.github_problem) && <p className="callout callout-danger" role="alert">{error || triage?.error}</p>}
    <div className="prws-body">
    <div className={`pr-workspace-layout${sidebarOpen ? "" : " pr-workspace-layout-collapsed"}`}>
      <aside className={`pr-workspace-context${sidebarOpen ? "" : " pr-workspace-context-collapsed"}`} aria-label="Pull request context">
        <div className="pr-workspace-context-heading">
          {sidebarOpen && <strong>{review ? `PR #${review.number}` : "Pull requests"}</strong>}
          <button type="button" className="btn btn-icon btn-ghost btn-sm" aria-label={sidebarOpen ? "Collapse PR sidebar" : "Expand PR sidebar"} aria-expanded={sidebarOpen} title={sidebarOpen ? "Collapse PR sidebar" : "Expand PR sidebar"} onClick={() => setSidebarOpen((open) => !open)}><Icon name={sidebarOpen ? "chevronLeft" : "chevronRight"} size={14} /></button>
        </div>
        {sidebarOpen && review && <><p className="small muted">{review.head_ref} · {review.head_sha.slice(0, 7)}</p><p className="small">{review.open_count} open of {review.total} review items</p><p className="small muted">Fetched {review.fetched_at}</p><button className="btn btn-secondary btn-sm" disabled={busy || triageBusy} onClick={() => void switchTab("threads")}>Review threads</button></>}
        {sidebarOpen && !review && <p className="muted small">Choose a PR in Threads to load its files and AI review.</p>}
        {!sidebarOpen && <span className="pr-workspace-context-badge" title={review ? `PR #${review.number}` : "Pull requests"}>{review ? `#${review.number}` : "PR"}</span>}
      </aside>
      <section className="pr-workspace-main">
        <nav className="pr-workspace-tabs" aria-label="PR workspace views">
          {(["files", "threads", "ai"] as const).map((item) => <button key={item} className={`btn ${tab === item ? "btn-primary" : "btn-secondary"}`} aria-current={tab === item ? "page" : undefined} disabled={!!tabLockReason(item)} title={tabLockReason(item) ?? undefined} onClick={() => void switchTab(item)}>{item === "ai" ? `AI Review${ai?.running ? " · Running" : ""}` : item[0].toUpperCase() + item.slice(1)}</button>)}
        </nav>
        {!review && tab === "threads" && <p className="muted">Pick a pull request from the list. Entries marked 'Selected for this feature' match this branch.</p>}
        {review && tab === "files" && workflowId && <PrReviewPanel key={`${workflowId}:${review.number}:${review.head_sha}`} projectId={target.project_id} onClose={() => undefined}
          embedded={{ workflowId, number: review.number, onDirtyChange: setFilesDirty, onOpenReview: () => void closeWorkspace(false, true), fallback: <ReadOnlyFiles workflowId={workflowId} /> }} />}
        {workflowId && <div className="pr-workspace-threads" hidden={tab !== "threads"}>
          <PrTriagePanel target={target} embedded embeddedActive={tab === "threads"} embeddedView={triage} onEmbeddedViewChange={setTriage} onEmbeddedBusyChange={setTriageBusy} onRequestClose={(close) => { triageClose.current = close; }} onClose={() => void closeWorkspace(true)} onHandoff={onHandoff} />
        </div>}
        {review && tab === "ai" && <AiReviewTab aiReview={aiReview} workspace={workspace} review={review} target={target} onHandoff={onHandoff} />}
      </section>
    </div>
    </div>
    <footer className="pr-workspace-footer"><span className="muted small">Files and AI findings are pinned to the selected PR head. Back to feature keeps this workspace open; Close workspace ends it.</span><button className="btn btn-secondary btn-sm" title="End this workspace and return to the feature" disabled={busy || triageBusy || ai?.running || !!ai?.precall || (filesDirty && tab === "files")} onClick={requestClose}>Close workspace</button></footer>
  </section>;
}
