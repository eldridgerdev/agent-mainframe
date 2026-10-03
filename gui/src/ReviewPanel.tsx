import { useEffect, useState } from "react";
import { DiffLine, ReviewAction, ReviewLocation, ReviewSeverity, ReviewSpan, ReviewView } from "./api";
import { Hunk } from "./DiffPanel";
import Markdown from "./Markdown";
import { Field, Modal, Spinner } from "./ui";

type Editor = { kind: "comment" | "reject" | "general" | "line_comment" | "suggestion"; span?: ReviewSpan; path: string; text: string; severity: ReviewSeverity; original: string; originalSeverity: ReviewSeverity };

function locationLabel(location: ReviewLocation) {
  return location.new_line !== null ? `line ${location.new_line}` : `base line ${location.old_line}`;
}

export default function ReviewPanel({ view, busy, error, onAct }: {
  view: ReviewView; busy: boolean; error: string | null; onAct: (action: ReviewAction) => Promise<boolean>;
}) {
  const [filter, setFilter] = useState("");
  const [split, setSplit] = useState(false);
  const [editor, setEditor] = useState<Editor | null>(null);
  const [selection, setSelection] = useState<{ anchor: ReviewLocation; cursor: ReviewLocation } | null>(null);
  const [pending, setPending] = useState<ReviewAction | "cancel" | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const dirty = editor !== null && (editor.text !== editor.original || editor.severity !== editor.originalSeverity);
  const file = view.files.find((f) => f.diff.path === view.selected_path);
  useEffect(() => { setSelection(null); setNotice(null); }, [view.workflow_id, view.revision, view.selected_path]);
  const lines = file?.diff.hunks.flatMap((hunk) => hunk.lines).filter((line) => line.kind !== "marker") ?? [];
  const locationKey = (location: ReviewLocation) => `${location.old_line}:${location.new_line}`;
  const lineIndices = new Map(lines.map((line, index) => [locationKey(line), index]));
  const lineIndex = (location: ReviewLocation) => lineIndices.get(locationKey(location)) ?? -1;
  function selectedSpan(): ReviewSpan | null {
    if (!selection) return null;
    return lineIndex(selection.anchor) <= lineIndex(selection.cursor)
      ? { start: selection.anchor, end: selection.cursor } : { start: selection.cursor, end: selection.anchor };
  }
  const span = selectedSpan();
  function selectLine(line: DiffLine, extend: boolean) {
    if (busy || dirty || pending) return;
    setEditor(null);
    setNotice(null);
    const location = { old_line: line.old_line, new_line: line.new_line };
    setSelection({ anchor: extend && selection ? selection.anchor : location, cursor: location });
  }
  function editSpan(kind: "line_comment" | "suggestion", selected: ReviewSpan) {
    if (!file || busy || dirty || pending) return;
    selected = { start: selected.start, end: selected.end };
    const lo = lineIndex(selected.start), hi = lineIndex(selected.end);
    // Saving replaces every thread overlapping the span and re-anchors the
    // result there, so only edit a thread at its own span: a selection inside
    // one thread snaps to it, and any other overlap is refused rather than
    // silently moving a suggestion or deleting a neighbouring thread.
    const overlapping = file.line_comments.filter((comment) => {
      const start = lineIndex(comment.start), end = lineIndex(comment.end);
      return start >= 0 && end >= 0 && start <= hi && end >= lo;
    });
    const existing = overlapping.length === 1 && overlapping[0].editable
      && lineIndex(overlapping[0].start) <= lo && lineIndex(overlapping[0].end) >= hi ? overlapping[0] : undefined;
    if (overlapping.length > 0 && !existing) {
      const anchors = overlapping.map((comment) => comment.anchor).join(", ");
      setNotice(overlapping.some((comment) => comment.anchor_lost)
        ? `Selection overlaps a thread whose anchor was lost (${anchors}); refresh changes before editing these lines.`
        : `Selection overlaps saved ${overlapping.length === 1 ? "thread" : "threads"} at ${anchors}, and saving would replace ${overlapping.length === 1 ? "it" : "them"}. Select lines within one thread to edit it, or lines clear of saved threads.`);
      return;
    }
    setNotice(null);
    if (existing) selected = { start: existing.start, end: existing.end };
    // Highlight exactly the lines the editor is anchored to.
    setSelection({ anchor: selected.start, cursor: selected.end });
    const text = kind === "line_comment" ? existing?.text ?? ""
      : existing?.suggestion ?? lines.slice(lineIndex(selected.start), lineIndex(selected.end) + 1).map((line) => line.text.slice(1)).join("\n");
    const severity = existing?.severity ?? "suggestion";
    setEditor({ kind, path: file.diff.path, span: selected, text, severity, original: text, originalSeverity: severity });
  }
  const files = view.files.filter((f) => f.diff.path.toLowerCase().includes(filter.toLowerCase()));
  const approved = view.files.filter((f) => f.verdict === "approved").length;
  const rejected = view.files.filter((f) => f.verdict === "rejected").length;

  async function run(action: ReviewAction | "cancel") {
    if (busy) return;
    if (action === "cancel" || await onAct(action)) { setEditor(null); setPending(null); }
  }
  function request(action: ReviewAction | "cancel") {
    if (busy) return;
    if (dirty || (action !== "cancel" && (action.kind === "discard" || (action.kind === "reload" && view.save_error)))) setPending(action);
    else void run(action);
  }
  function edit(kind: "comment" | "reject" | "general") {
    if (busy || dirty) return;
    const text = kind === "general" ? view.general_feedback : kind === "reject" ? file?.feedback ?? "" : file?.comment?.text ?? "";
    // Like the TUI, a fresh rejection is a must-fix signal; an existing one keeps its severity.
    const severity = kind === "reject" ? (file?.verdict === "rejected" ? file.severity : "blocker") : file?.comment?.severity ?? "suggestion";
    setEditor({ kind, path: file?.diff.path ?? "", text, severity, original: text, originalSeverity: severity });
  }
  async function submit() {
    if (!editor || busy) return;
    const action: ReviewAction = editor.kind === "general" ? { kind: "general", text: editor.text }
      : editor.kind === "reject" ? { kind: "reject", path: editor.path, feedback: editor.text, severity: editor.severity }
      : editor.kind === "line_comment" ? { kind: "line_comment", path: editor.path, ...editor.span!, text: editor.text, severity: editor.severity }
      : editor.kind === "suggestion" ? { kind: "suggestion", path: editor.path, ...editor.span!, text: editor.text }
      : { kind: "comment", path: editor.path, text: editor.text, severity: editor.severity };
    if (await onAct(action)) setEditor(null);
  }
  return <Modal label="Final Review" title={`Final Review · ${view.feature_name}`} size="xl" onClose={() => request({ kind: "pause" })}
    footer={<button className="btn btn-secondary" disabled={busy} onClick={() => request({ kind: "pause" })}>Pause review</button>}>
    <p>Review progress is shared with the TUI. Pause and reopen to resume. Finish and send feedback from the TUI.</p>
    <p className="diff-summary">{view.branch} · {view.base_ref} · {approved} approved · {rejected} rejected · {view.files.length - approved - rejected} undecided</p>
    {busy && <p role="status"><Spinner /> Updating review…</p>}
    {(error || view.error) && <p role="alert">{error || view.error}</p>}
    {view.save_error && <div role="alert"><p>Progress was not saved: {view.save_error}</p><button className="btn btn-secondary" disabled={busy} onClick={() => void onAct({ kind: "retry_save" })}>Retry save</button>
      <button className="btn btn-ghost" disabled={busy || pending !== null} onClick={() => request({ kind: "discard" })}>Close without saving</button></div>}
    {pending && (pending !== "cancel" && pending.kind === "discard"
      ? <div className="review-confirm" role="alertdialog" aria-label="Close review without saving">
        <p>Close without saving? Changes since the last successful save are lost.</p>
        <button className="btn btn-danger" disabled={busy} onClick={() => void run(pending)}>Discard and close</button>
        <button className="btn btn-secondary" disabled={busy} onClick={() => setPending(null)}>Keep editing</button>
      </div>
      : <div className="review-confirm" role="alertdialog" aria-label="Discard unsaved review draft">
        <p>Discard unsaved edits and continue?</p>
        <button className="btn btn-danger" disabled={busy} onClick={() => void run(pending)}>Discard and continue</button>
        <button className="btn btn-secondary" disabled={busy} onClick={() => setPending(null)}>Keep editing</button>
      </div>)}
    <div className="diff-controls">
      <Field label="Filter review files"><input value={filter} onChange={(event) => setFilter(event.target.value)} /></Field>
      <Field label="Review layout"><select value={split ? "split" : "unified"} onChange={(event) => setSplit(event.target.value === "split")}><option value="unified">Unified</option><option value="split">Side by side</option></select></Field>
      <button className="btn btn-secondary btn-sm" disabled={busy || pending !== null} onClick={() => request({ kind: "refresh" })}>Refresh changes</button>
      <button className="btn btn-ghost btn-sm" disabled={busy || pending !== null} onClick={() => request({ kind: "reload" })}>Reload saved review</button>
      <button className="btn btn-ghost btn-sm" disabled={busy || pending !== null || view.error !== null} onClick={() => request({ kind: "undo" })}>Undo verdict</button>
      <button className="btn btn-secondary btn-sm" disabled={busy || dirty || pending !== null} onClick={() => edit("general")}>Overall feedback</button>
    </div>
    {view.general_feedback && <section aria-label="Saved overall feedback"><Markdown source={view.general_feedback} /></section>}
    {editor && <form className="review-editor" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <Field label={editor.kind === "general" ? "Overall feedback draft" : editor.kind === "reject" ? "Rejection feedback" : editor.kind === "line_comment" ? "Line comment" : editor.kind === "suggestion" ? "Suggested replacement" : "File comment"}>
        <textarea rows={5} value={editor.text} disabled={busy} onChange={(event) => setEditor({ ...editor, text: event.target.value })} />
      </Field>
      {editor.kind !== "general" && editor.kind !== "suggestion" && <Field label="Severity"><select value={editor.severity} disabled={busy} onChange={(event) => setEditor({ ...editor, severity: event.target.value as ReviewSeverity })}>
        {["blocker", "suggestion", "nit", "question", "praise"].map((severity) => <option key={severity} value={severity}>{severity}</option>)}
      </select></Field>}
      <button className="btn btn-primary" disabled={busy || pending !== null}>Save {editor.kind === "reject" ? "rejection" : editor.kind === "general" ? "overall feedback" : editor.kind === "suggestion" ? "suggestion" : "comment"}</button>
      <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => request("cancel")}>Cancel edit</button>
      {editor.span && <p className="muted small">Anchored to {locationLabel(editor.span.start)} – {locationLabel(editor.span.end)}. An empty save removes this {editor.kind === "suggestion" ? "suggestion" : "comment's prose"}; the other part of the thread is kept.</p>}
      {editor.kind === "comment" && <p className="muted small">An empty comment removes the saved file comment. File comments leave the verdict unchanged.</p>}
    </form>}
    {!view.error && <div className="diff-reader">
      <nav className="diff-files" aria-label="Review files">{files.map((item) => <button key={item.diff.path} disabled={busy || pending !== null} aria-pressed={item.diff.path === view.selected_path}
        className={`diff-file ${item.diff.path === view.selected_path ? "diff-file-selected" : ""}`} onClick={() => request({ kind: "select", path: item.diff.path })}>
        <span>{item.diff.path}</span><small>{item.verdict}{item.changed_since_last && " · changed since last review"}</small>
      </button>)}</nav>
      <section className="diff-content" aria-label="Review file">
        {file ? <>
          <div className="diff-file-header"><strong>{file.diff.old_path && file.diff.old_path !== file.diff.path ? `${file.diff.old_path} → ${file.diff.path}` : file.diff.path}</strong><span>{file.verdict}</span>
            <button className="btn btn-secondary btn-sm" disabled={busy || pending !== null} onClick={() => request({ kind: "approve", path: file.diff.path })}>Approve file</button>
            <button className="btn btn-secondary btn-sm" disabled={busy || pending !== null} onClick={() => request({ kind: "skip", path: file.diff.path })}>Skip file</button>
            <button className="btn btn-secondary btn-sm" disabled={busy || dirty || pending !== null} onClick={() => edit("reject")}>Reject file</button>
            <button className="btn btn-ghost btn-sm" disabled={busy || dirty || pending !== null} onClick={() => edit("comment")}>Edit file comment</button>
          </div>
          {file.feedback && <p className="review-note">[{file.severity}] {file.feedback}</p>}
          {file.notes && <details className="review-note"><summary>Developer notes</summary><Markdown source={file.notes} /></details>}
          {file.comment && <div className="review-note"><p>[{file.comment.severity}] {file.comment.text}{file.comment.resolved && " (resolved)"}{file.comment.carried && " (previous round)"}</p>
            <button className="btn btn-ghost btn-sm" disabled={busy || pending !== null} onClick={() => request({ kind: "toggle_resolved", path: file.diff.path })}>{file.comment.resolved ? "Reopen comment" : "Resolve comment"}</button>
          </div>}
          {file.line_comments.length > 0 && <details className="review-note"><summary>Saved line comments ({file.line_comments.length})</summary>
            {file.line_comments.map((comment, index) => <div key={index}><p>{comment.anchor} [{comment.severity}] {comment.text}{comment.resolved && " (resolved)"}{comment.draft && " (AI draft)"}{comment.anchor_lost && " (anchor lost)"}</p>{comment.suggestion !== null && <pre>{comment.suggestion}</pre>}
              <button className="btn btn-ghost btn-sm" disabled={busy || dirty || pending !== null || !comment.editable} onClick={() => editSpan("line_comment", comment)}>Edit line comment</button>
              <button className="btn btn-ghost btn-sm" disabled={busy || dirty || pending !== null || !comment.editable} onClick={() => editSpan("suggestion", comment)}>Edit suggestion</button>
              <button className="btn btn-ghost btn-sm" disabled={busy || pending !== null || !comment.editable || comment.draft} onClick={() => request({ kind: "toggle_line_resolved", path: file.diff.path, start: comment.start, end: comment.end })}>{comment.resolved ? "Reopen thread" : "Resolve thread"}</button>
            </div>)}
          </details>}
          {!file.diff.is_binary && lines.length > 0 && <div className="review-line-controls">
            <p className="muted small">Click a line number to select it; Shift-click another to select a range in diff order.</p>
            {span && <p>Selected {locationLabel(span.start)} – {locationLabel(span.end)}</p>}
            {notice && <p role="alert">{notice}</p>}
            <button className="btn btn-secondary btn-sm" disabled={busy || dirty || pending !== null || !span} onClick={() => span && editSpan("line_comment", span)}>Comment on selection</button>
            <button className="btn btn-secondary btn-sm" disabled={busy || dirty || pending !== null || !span} onClick={() => span && editSpan("suggestion", span)}>Suggest replacement</button>
          </div>}
          <div className="diff-code">
            {file.diff.is_binary ? <p>Binary file changed; no text diff is available.</p> : file.diff.hunks.length === 0 ? <pre>{file.diff.patch || "No textual changes."}</pre>
              : file.diff.hunks.map((hunk, index) => <Hunk key={index} hunk={hunk} split={split} selection={{ disabled: busy || dirty || pending !== null, select: selectLine,
                contains: (line) => !!span && line.kind !== "marker" && lineIndex(line) >= lineIndex(span.start) && lineIndex(line) <= lineIndex(span.end) }} />)}
          </div>
        </> : <p>{view.files.length ? "Select a file to review." : "No changes to review."}</p>}
      </section>
    </div>}
  </Modal>;
}
