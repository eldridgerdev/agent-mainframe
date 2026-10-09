import { useEffect, useRef, useState } from "react";
import { asGuiError } from "./api";
import { Hunk } from "./DiffPanel";
import Markdown from "./Markdown";
import { Field, Modal, Spinner } from "./ui";
import { PrReviewAction, PrReviewView, prReviewAct, prReviewBegin, prReviewSnapshot } from "./prReviewApi";

export default function PrReviewPanel({ projectId, onClose }: { projectId: string; onClose: () => void }) {
  const [view, setView] = useState<PrReviewView | null>(null);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [path, setPath] = useState<string | null>(null);
  const [comment, setComment] = useState("");
  const [summary, setSummary] = useState("");
  const [event, setEvent] = useState("COMMENT");
  const [discard, setDiscard] = useState<"back" | "close" | null>(null);
  const pending = useRef(false);
  useEffect(() => {
    let live = true;
    prReviewBegin(projectId).then((next) => { if (live) setView(next); })
      .catch((err) => { if (live) setError(asGuiError(err).message); })
      .finally(() => { if (live) setBusy(false); });
    return () => { live = false; };
  }, [projectId]);
  const waiting = !!view?.loading || !!view?.submission?.posting;
  useEffect(() => {
    if (!view || !waiting || busy) return;
    const id = view.workflow_id;
    const timer = window.setInterval(() => {
      void prReviewSnapshot(id).then((next) => {
        setView((current) => current?.workflow_id === id && next.revision >= current.revision && !pending.current ? next : current);
      }).catch((err) => setError(asGuiError(err).message));
    }, 700);
    return () => window.clearInterval(timer);
  }, [view?.workflow_id, waiting, busy]);
  const file = view?.files.find((f) => f.diff.path === path) ?? view?.files[0];
  useEffect(() => { setComment(file?.comment ?? ""); }, [view?.number, file?.diff.path, file?.comment]);
  useEffect(() => { setSummary(view?.summary ?? ""); }, [view?.number, view?.summary]);
  const dirty = comment !== (file?.comment ?? "") || summary !== (view?.summary ?? "");
  const locked = busy || !!view?.submission;
  async function act(action: PrReviewAction) {
    if (!view || pending.current) return;
    pending.current = true;
    setBusy(true);
    try {
      const next = await prReviewAct(view, action);
      setError(null);
      setDiscard(null);
      if (next) setView(next); else onClose();
    } catch (err) {
      setError(asGuiError(err).message);
      try { setView(await prReviewSnapshot(view.workflow_id)); } catch { /* retain editors */ }
    } finally { pending.current = false; setBusy(false); }
  }
  function leave(kind: "back" | "close") {
    if (dirty) setDiscard(kind); else if (view) void act({ kind }); else onClose();
  }
  return <Modal label="PR Review" title={`PR Review${view ? ` · ${view.project_name}` : ""}`} size="xl"
    dismissable={!locked} onClose={() => leave("close")}>
    {(error || view?.error) && <p className="callout callout-danger" role="alert">{error || view?.error}</p>}
    {view?.notice && <p role="status">{view.notice}</p>}
    {discard && <div className="callout callout-warning"><p>Discard unsaved editor text? Saved review drafts are kept.</p>
      <button className="btn btn-secondary" onClick={() => setDiscard(null)}>Keep editing</button>
      <button className="btn btn-warning" onClick={() => void act({ kind: discard })}>Discard and leave</button>
    </div>}
    {(!view || waiting) && <p role="status"><Spinner /> Loading pull requests…</p>}
    {view?.stage === "pick" && <section aria-label="Choose a pull request">
      <button className="btn btn-secondary" disabled={busy} onClick={() => void act({ kind: "retry" })}>Refresh pull requests</button>
      {!waiting && !view.error && view.entries.length === 0 && <p>No open pull requests.</p>}
      <ul className="pr-list">{view.entries.map((entry) => <li key={entry.number}>
        <button className="pr-entry" disabled={busy || waiting} onClick={() => void act({ kind: "open", number: entry.number })}>
          <span className="row"><strong>#{entry.number}</strong> {entry.title}</span>
          <span className="row small muted">{entry.head_ref} · {entry.author}
            {entry.is_draft && <span className="tag">Draft PR</span>}
            {entry.has_draft && <span className="tag">Saved review</span>}
            {entry.updated && <span className="tag">Updated since review</span>}
          </span>
        </button>
      </li>)}</ul>
    </section>}
    {view?.stage === "review" && <>
      <h3>#{view.number} {view.title}</h3>
      <div className="diff-reader">
        <nav className="diff-files" aria-label="Changed files">{view.files.map((f) => <button key={f.diff.path}
          className={`diff-file ${file === f ? "diff-file-selected" : ""}`} disabled={locked || comment !== (file?.comment ?? "")}
          onClick={() => setPath(f.diff.path)}>{f.diff.path}<small>+{f.diff.additions} −{f.diff.deletions}</small></button>)}</nav>
        <section className="diff-content" aria-label="File diff">{file && <>
          <strong>{file.diff.path}</strong>
          <div className="diff-code">{file.diff.is_binary ? <p>Binary file changed.</p> : file.diff.hunks.length ? file.diff.hunks.map((hunk, index) => <Hunk key={index} hunk={hunk} split={false} />) : <pre>{file.diff.patch || "No textual changes."}</pre>}</div>
        </>}</section>
      </div>
      {file && <><Field label={`Comment on ${file.diff.path}`}><textarea value={comment} disabled={locked} onChange={(e) => setComment(e.target.value)} /></Field>
        <button className="btn btn-secondary" disabled={locked || comment === file.comment} onClick={() => void act({ kind: "file_comment", path: file.diff.path, text: comment })}>Save file comment</button></>}
      <Field label="Review summary"><textarea value={summary} disabled={locked} onChange={(e) => setSummary(e.target.value)} /></Field>
      <button className="btn btn-secondary" disabled={locked || summary === view.summary} onClick={() => void act({ kind: "summary", text: summary })}>Save summary</button>
      <Field label="Review event"><select value={event} disabled={locked} onChange={(e) => setEvent(e.target.value)}>
        <option value="COMMENT">Comment</option><option value="APPROVE">Approve</option><option value="REQUEST_CHANGES">Request changes</option>
      </select></Field>
      <div className="row"><button className="btn btn-secondary" disabled={locked} onClick={() => leave("back")}>Back to pull requests</button>
        <button className="btn btn-primary" disabled={locked || dirty} onClick={() => void act({ kind: "preview", event })}>Preview submission</button></div>
    </>}
    {view?.submission && <section aria-label="Confirm PR review" className="callout">
      <h3>Post {view.submission.event.replaceAll("_", " ").toLowerCase()} review to PR #{view.number}</h3>
      <Markdown source={view.submission.body || "No summary."} />
      {view.submission.comments.map((c, i) => <div key={i}><strong>{c.path}:{c.line}</strong><Markdown source={c.body} /></div>)}
      {view.submission.file_comments.map((c, i) => <div key={i}><strong>{c.path}</strong><Markdown source={c.body} /></div>)}
      {view.submission.error && <p role="alert">{view.submission.error}</p>}
      <button className="btn btn-secondary" disabled={busy || view.submission.posting} onClick={() => void act({ kind: "cancel_submit" })}>Keep editing</button>
      {view.submission.head_moved ? <button className="btn btn-primary" disabled={busy} onClick={() => void act({ kind: "reopen" })}>Reopen updated PR</button>
        : <button className="btn btn-primary" disabled={busy || view.submission.posting} onClick={() => void act({ kind: "confirm_submit" })}>{view.submission.posting ? "Posting…" : "Confirm and post to GitHub"}</button>}
    </section>}
  </Modal>;
}
