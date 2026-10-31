import { useEffect, useRef, useState } from "react";
import { asGuiError } from "./api";
import { DiffLine, ReviewLocation, ReviewSeverity, ReviewSpan } from "./api";
import { Hunk } from "./DiffPanel";
import Markdown from "./Markdown";
import { Field, Modal, Spinner } from "./ui";
import { PrReviewAction, PrReviewThread, PrReviewView, prReviewAct, prReviewBegin, prReviewSnapshot } from "./prReviewApi";

type Editor = { kind: "line_comment" | "suggestion"; span: ReviewSpan; path: string; text: string; severity: ReviewSeverity; original: string; originalSeverity: ReviewSeverity };
const SEVERITIES: ReviewSeverity[] = ["blocker", "suggestion", "nit", "question", "praise"];
const locationLabel = (l: ReviewLocation) => l.new_line !== null ? `line ${l.new_line}` : `base line ${l.old_line}`;
const locationKey = (l: ReviewLocation) => `${l.old_line}:${l.new_line}`;

export default function PrReviewPanel({ projectId, onClose }: { projectId: string; onClose: () => void }) {
  const [view, setView] = useState<PrReviewView | null>(null);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [path, setPath] = useState<string | null>(null);
  const [comment, setComment] = useState("");
  const [summary, setSummary] = useState("");
  const [event, setEvent] = useState("COMMENT");
  const [discard, setDiscard] = useState<"back" | "close" | null>(null);
  const [selection, setSelection] = useState<{ anchor: ReviewLocation; cursor: ReviewLocation } | null>(null);
  const [editor, setEditor] = useState<Editor | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
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
  const editorDirty = !!editor && (editor.text !== editor.original || editor.severity !== editor.originalSeverity);
  const dirty = comment !== (file?.comment ?? "") || summary !== (view?.summary ?? "") || editorDirty;
  const locked = busy || !!view?.submission;
  // Selections and editors belong to one file of one PR head.
  useEffect(() => { setSelection(null); setEditor(null); setNotice(null); }, [view?.number, view?.head_sha, file?.diff.path]);
  const lines = file?.diff.hunks.flatMap((hunk) => hunk.lines).filter((line) => line.kind !== "marker") ?? [];
  const lineIndices = new Map(lines.map((line, index) => [locationKey(line), index]));
  const lineIndex = (l: ReviewLocation) => lineIndices.get(locationKey(l)) ?? -1;
  // Hunks of the PR's own diff; expanded context is not part of it.
  const prHunks = new Map((file?.pr_lines ?? []).map((l) => [locationKey(l), l.hunk]));
  const hunkIndex = (l: ReviewLocation) => prHunks.get(locationKey(l)) ?? -1;
  const span: ReviewSpan | null = !selection ? null : lineIndex(selection.anchor) <= lineIndex(selection.cursor)
    ? { start: selection.anchor, end: selection.cursor } : { start: selection.cursor, end: selection.anchor };
  function selectLine(line: DiffLine, extend: boolean) {
    if (locked || editorDirty) return;
    setEditor(null);
    setNotice(null);
    const location = { old_line: line.old_line, new_line: line.new_line };
    setSelection({ anchor: extend && selection ? selection.anchor : location, cursor: location });
  }
  function editSpan(kind: Editor["kind"], selected: ReviewSpan) {
    if (!file || locked || editorDirty) return;
    selected = { start: selected.start, end: selected.end };
    const lo = lineIndex(selected.start), hi = lineIndex(selected.end);
    // Saving replaces every thread overlapping the span, so only edit a thread
    // at its own span: a selection inside one snaps to it, other overlap is refused.
    const overlapping = file.line_comments.filter((c) => {
      const s = lineIndex(c.start), e = lineIndex(c.end);
      return s >= 0 && e >= 0 && s <= hi && e >= lo;
    });
    const existing: PrReviewThread | undefined = overlapping.length === 1 && overlapping[0].editable
      && lineIndex(overlapping[0].start) <= lo && lineIndex(overlapping[0].end) >= hi ? overlapping[0] : undefined;
    if (overlapping.length > 0 && !existing) {
      setNotice(`Selection overlaps saved ${overlapping.length === 1 ? "thread" : "threads"} at ${overlapping.map((c) => c.anchor).join(", ")}, and saving would replace ${overlapping.length === 1 ? "it" : "them"}. Select lines within one thread to edit it, or lines clear of saved threads.`);
      return;
    }
    if (hunkIndex(selected.start) < 0 || hunkIndex(selected.end) < 0) {
      setNotice("GitHub takes comments on the PR's own diff lines; these lines were only reached by expanding context.");
      return;
    }
    if (hunkIndex(selected.start) !== hunkIndex(selected.end)) {
      setNotice("GitHub only accepts a range inside one hunk; select lines within a single hunk.");
      return;
    }
    if (kind === "suggestion" && lines.slice(lo, hi + 1).some((line) => line.new_line === null)) {
      setNotice("A suggestion replaces current lines; the selection includes a removed (base-only) line, so select current lines or comment instead.");
      return;
    }
    setNotice(null);
    if (existing) selected = { start: existing.start, end: existing.end };
    setSelection({ anchor: selected.start, cursor: selected.end });
    const text = kind === "line_comment" ? existing?.text ?? ""
      : existing?.suggestion ?? lines.slice(lineIndex(selected.start), lineIndex(selected.end) + 1).map((line) => line.text.slice(1)).join("\n");
    const severity = existing?.severity ?? "suggestion";
    setEditor({ kind, span: selected, path: file.diff.path, text, severity, original: text, originalSeverity: severity });
  }
  async function submitEditor() {
    if (!editor || locked) return;
    const base = { path: editor.path, ...editor.span, text: editor.text };
    if (await act(editor.kind === "line_comment" ? { kind: "line_comment", ...base, severity: editor.severity } : { kind: "suggestion", ...base })) {
      setEditor(null);
    }
  }
  async function act(action: PrReviewAction): Promise<boolean> {
    if (!view || pending.current) return false;
    pending.current = true;
    setBusy(true);
    try {
      const next = await prReviewAct(view, action);
      setError(null);
      setDiscard(null);
      if (next) setView(next); else onClose();
      return true;
    } catch (err) {
      setError(asGuiError(err).message);
      try {
        const next = await prReviewSnapshot(view.workflow_id);
        // Failed draft writes can leave unpersisted text in the backend snapshot.
        // Refresh the revision for retries, but retain the last saved baseline.
        if (action.kind === "summary") next.summary = view.summary;
        if (action.kind === "file_comment") next.files = next.files.map((f) =>
          f.diff.path === action.path ? { ...f, comment: view.files.find((saved) => saved.diff.path === action.path)?.comment ?? "" } : f);
        setView(next);
      } catch { /* retain editors */ }
      return false;
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
      {view.head_sha && <p className="muted small">Comments are pinned to head <code>{view.head_sha.slice(0, 8)}</code>; posting is refused if the PR moves.</p>}
      <div className="diff-reader">
        <nav className="diff-files" aria-label="Changed files">{view.files.map((f) => <button key={f.diff.path}
          className={`diff-file ${file === f ? "diff-file-selected" : ""}`} disabled={locked || comment !== (file?.comment ?? "") || editorDirty}
          onClick={() => setPath(f.diff.path)}>{f.diff.path}<small>+{f.diff.additions} −{f.diff.deletions}</small></button>)}</nav>
        <section className="diff-content" aria-label="File diff">{file && <>
          <strong>{file.diff.path}</strong>
          {!file.diff.is_binary && lines.length > 0 && <div className="review-line-controls">
            <p className="muted small">Click a line number to select it; Shift-click another to select a range inside one hunk. Base-only (removed) lines comment on the base side.</p>
            {span && <p>Selected {locationLabel(span.start)} – {locationLabel(span.end)}</p>}
            {notice && <p role="alert">{notice}</p>}
            <button className="btn btn-secondary btn-sm" disabled={locked || editorDirty || !span} onClick={() => span && editSpan("line_comment", span)}>Comment on selection</button>
            <button className="btn btn-secondary btn-sm" disabled={locked || editorDirty || !span} onClick={() => span && editSpan("suggestion", span)}>Suggest replacement</button>
          </div>}
          {editor && <form className="review-editor" onSubmit={(event) => { event.preventDefault(); void submitEditor(); }}>
            <Field label={editor.kind === "line_comment" ? "Line comment" : "Suggested replacement"}>
              <textarea rows={5} value={editor.text} disabled={locked} onChange={(event) => setEditor({ ...editor, text: event.target.value })} />
            </Field>
            {editor.kind === "line_comment" && <Field label="Severity"><select value={editor.severity} disabled={locked} onChange={(event) => setEditor({ ...editor, severity: event.target.value as ReviewSeverity })}>
              {SEVERITIES.map((severity) => <option key={severity} value={severity}>{severity}</option>)}
            </select></Field>}
            <button className="btn btn-primary" disabled={locked}>Save {editor.kind === "line_comment" ? "line comment" : "suggestion"}</button>
            <button type="button" className="btn btn-secondary" disabled={locked} onClick={() => setEditor(null)}>Cancel edit</button>
            <p className="muted small">Anchored to {locationLabel(editor.span.start)} – {locationLabel(editor.span.end)}. An empty save removes this {editor.kind === "suggestion" ? "suggestion" : "comment's prose"}; the other part of the thread is kept. Posts only after you confirm the submission preview.</p>
          </form>}
          <div className="diff-code">{file.diff.is_binary ? <p>Binary file changed.</p> : file.diff.hunks.length ? file.diff.hunks.map((hunk, index) => <Hunk key={index} hunk={hunk} split={false} selection={{ disabled: locked || editorDirty, select: selectLine,
            contains: (line) => !!span && line.kind !== "marker" && lineIndex(line) >= lineIndex(span.start) && lineIndex(line) <= lineIndex(span.end) }} />) : <pre>{file.diff.patch || "No textual changes."}</pre>}</div>
          {file.line_comments.length > 0 && <details className="review-note" open><summary>Saved line comments ({file.line_comments.length})</summary>
            {file.line_comments.map((thread, index) => <div key={index}>
              <p>{thread.anchor} · {thread.side === "LEFT" ? "base" : "current"} · {thread.side} [{thread.severity}] {thread.text}{thread.resolved && " (resolved)"}{thread.anchor_lost && " (anchor lost)"}</p>
              {thread.suggestion !== null && <pre>{thread.suggestion}</pre>}
              {thread.note && <p className="muted small">{thread.note}</p>}
              <button className="btn btn-ghost btn-sm" disabled={locked || dirty || !thread.editable} onClick={() => editSpan("line_comment", thread)}>Edit line comment</button>
              <button className="btn btn-ghost btn-sm" disabled={locked || dirty || !thread.editable} onClick={() => editSpan("suggestion", thread)}>Edit suggestion</button>
              <button className="btn btn-ghost btn-sm" disabled={locked || dirty || !thread.editable}
                onClick={() => void act({ kind: "toggle_line_resolved", path: file.diff.path, start: thread.start, end: thread.end })}>{thread.resolved ? "Reopen thread" : "Resolve thread"}</button>
            </div>)}
          </details>}
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
      {view.submission.comments.map((c, i) => <div key={i}>
        <strong>{c.path}:{c.start_line != null ? `${c.start_line}–${c.line}` : c.line} ({c.side === "LEFT" ? "base" : "current"} · {c.side})</strong>
        <Markdown source={c.body} />
      </div>)}
      {view.submission.file_comments.map((c, i) => <div key={i}><strong>{c.path}</strong><Markdown source={c.body} /></div>)}
      {view.submission.error && <p role="alert">{view.submission.error}</p>}
      <button className="btn btn-secondary" disabled={busy || view.submission.posting} onClick={() => void act({ kind: "cancel_submit" })}>Keep editing</button>
      {view.submission.head_moved ? <button className="btn btn-primary" disabled={busy} onClick={() => void act({ kind: "reopen" })}>Reopen updated PR</button>
        : <button className="btn btn-primary" disabled={busy || view.submission.posting} onClick={() => void act({ kind: "confirm_submit" })}>{view.submission.posting ? "Posting…" : "Confirm and post to GitHub"}</button>}
    </section>}
  </Modal>;
}
