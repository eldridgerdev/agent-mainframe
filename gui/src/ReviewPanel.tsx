import { useState } from "react";
import { ReviewAction, ReviewSeverity, ReviewView } from "./api";
import { Hunk } from "./DiffPanel";
import Markdown from "./Markdown";
import { Field, Modal, Spinner } from "./ui";

type Editor = { kind: "comment" | "reject" | "general"; path: string; text: string; severity: ReviewSeverity; original: string; originalSeverity: ReviewSeverity };

export default function ReviewPanel({ view, busy, error, onAct }: {
  view: ReviewView; busy: boolean; error: string | null; onAct: (action: ReviewAction) => Promise<boolean>;
}) {
  const [filter, setFilter] = useState("");
  const [split, setSplit] = useState(false);
  const [editor, setEditor] = useState<Editor | null>(null);
  const [pending, setPending] = useState<ReviewAction | "cancel" | null>(null);
  const dirty = editor !== null && (editor.text !== editor.original || editor.severity !== editor.originalSeverity);
  const file = view.files.find((f) => f.diff.path === view.selected_path);
  const files = view.files.filter((f) => f.diff.path.toLowerCase().includes(filter.toLowerCase()));
  const approved = view.files.filter((f) => f.verdict === "approved").length;
  const rejected = view.files.filter((f) => f.verdict === "rejected").length;

  async function run(action: ReviewAction | "cancel") {
    if (busy) return;
    if (action === "cancel" || await onAct(action)) { setEditor(null); setPending(null); }
  }
  function request(action: ReviewAction | "cancel") {
    if (busy) return;
    if (dirty || (action !== "cancel" && action.kind === "reload" && view.save_error)) setPending(action);
    else void run(action);
  }
  function edit(kind: Editor["kind"]) {
    if (busy || dirty) return;
    const text = kind === "general" ? view.general_feedback : kind === "reject" ? file?.feedback ?? "" : file?.comment?.text ?? "";
    const severity = kind === "reject" ? file?.severity ?? "suggestion" : file?.comment?.severity ?? "suggestion";
    setEditor({ kind, path: file?.diff.path ?? "", text, severity, original: text, originalSeverity: severity });
  }
  async function submit() {
    if (!editor || busy) return;
    const action: ReviewAction = editor.kind === "general" ? { kind: "general", text: editor.text }
      : editor.kind === "reject" ? { kind: "reject", path: editor.path, feedback: editor.text, severity: editor.severity }
      : { kind: "comment", path: editor.path, text: editor.text, severity: editor.severity };
    if (await onAct(action)) setEditor(null);
  }
  return <Modal label="Final Review" title={`Final Review · ${view.feature_name}`} size="xl" onClose={() => request({ kind: "pause" })}
    footer={<button className="btn btn-secondary" disabled={busy} onClick={() => request({ kind: "pause" })}>Pause review</button>}>
    <p>Review progress is shared with the TUI. Pause and reopen to resume. Finish and send feedback from the TUI.</p>
    <p className="diff-summary">{view.branch} · {view.base_ref} · {approved} approved · {rejected} rejected · {view.files.length - approved - rejected} undecided</p>
    {busy && <p role="status"><Spinner /> Updating review…</p>}
    {(error || view.error) && <p role="alert">{error || view.error}</p>}
    {view.save_error && <div role="alert"><p>Progress was not saved: {view.save_error}</p><button className="btn btn-secondary" disabled={busy} onClick={() => void onAct({ kind: "retry_save" })}>Retry save</button></div>}
    {pending && <div className="review-confirm" role="alertdialog" aria-label="Discard unsaved review draft">
      <p>Discard unsaved edits and continue?</p>
      <button className="btn btn-danger" disabled={busy} onClick={() => void run(pending)}>Discard and continue</button>
      <button className="btn btn-secondary" disabled={busy} onClick={() => setPending(null)}>Keep editing</button>
    </div>}
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
      <Field label={editor.kind === "general" ? "Overall feedback draft" : editor.kind === "reject" ? "Rejection feedback" : "File comment"}>
        <textarea rows={5} value={editor.text} disabled={busy} onChange={(event) => setEditor({ ...editor, text: event.target.value })} />
      </Field>
      {editor.kind !== "general" && <Field label="Severity"><select value={editor.severity} disabled={busy} onChange={(event) => setEditor({ ...editor, severity: event.target.value as ReviewSeverity })}>
        {["blocker", "suggestion", "nit", "question", "praise"].map((severity) => <option key={severity} value={severity}>{severity}</option>)}
      </select></Field>}
      <button className="btn btn-primary" disabled={busy || pending !== null}>Save {editor.kind === "reject" ? "rejection" : editor.kind === "general" ? "overall feedback" : "comment"}</button>
      <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => request("cancel")}>Cancel edit</button>
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
          {file.line_comments.length > 0 && <details className="review-note"><summary>Saved line comments ({file.line_comments.length}) · edit in TUI</summary>
            {file.line_comments.map((comment, index) => <div key={index}><p>{comment.anchor} [{comment.severity}] {comment.text}{comment.resolved && " (resolved)"}{comment.draft && " (AI draft)"}{comment.anchor_lost && " (anchor lost)"}</p>{comment.suggestion !== null && <pre>{comment.suggestion}</pre>}</div>)}
          </details>}
          <div className="diff-code">
            {file.diff.is_binary ? <p>Binary file changed; no text diff is available.</p> : file.diff.hunks.length === 0 ? <pre>{file.diff.patch || "No textual changes."}</pre>
              : file.diff.hunks.map((hunk, index) => <Hunk key={index} hunk={hunk} split={split} />)}
          </div>
        </> : <p>{view.files.length ? "Select a file to review." : "No changes to review."}</p>}
      </section>
    </div>}
  </Modal>;
}
