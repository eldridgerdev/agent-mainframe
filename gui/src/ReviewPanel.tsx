import { useEffect, useRef, useState } from "react";
import { AgentSlug, DiffLine, ReviewAction, ReviewLocation, ReviewSeverity, ReviewSpan, ReviewView } from "./api";
import { Hunk } from "./DiffPanel";
import Markdown from "./Markdown";
import ReviewSummaryPanel from "./ReviewSummaryPanel";
import { Field, Modal, Spinner } from "./ui";

type Editor = { kind: "comment" | "reject" | "general" | "line_comment" | "suggestion"; span?: ReviewSpan; path: string; text: string; severity: ReviewSeverity; original: string; originalSeverity: ReviewSeverity };

function locationLabel(location: ReviewLocation) {
  return location.new_line !== null ? `line ${location.new_line}` : `base line ${location.old_line}`;
}

export default function ReviewPanel({ view, busy: commandBusy, error, onAct, onEditPrompt }: {
  view: ReviewView; busy: boolean; error: string | null; onAct: (action: ReviewAction) => Promise<boolean>;
  /** Opens the prompt override manager on the pending call's prompt. */
  onEditPrompt?: () => void;
}) {
  const ai = view.ai;
  const history = view.history;
  const summary = view.summary;
  const busy = commandBusy || view.check?.status === "running" || ai.precall !== null || (ai.question_running && ai.comment_draft !== null);
  const [question, setQuestion] = useState<{ path: string; span: ReviewSpan | null; text: string; harness: AgentSlug; turn?: number } | null>(null);
  const [commentDraft, setCommentDraft] = useState<{ request: number; text: string } | null>(null);
  const receivedDraft = useRef("");
  const receivedEditor = useRef("");
  const [filter, setFilter] = useState("");
  const [split, setSplit] = useState(false);
  const [editor, setEditor] = useState<Editor | null>(null);
  const [selection, setSelection] = useState<{ anchor: ReviewLocation; cursor: ReviewLocation } | null>(null);
  const [pending, setPending] = useState<ReviewAction | "cancel" | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const dirty = (editor !== null && (editor.text !== editor.original || editor.severity !== editor.originalSeverity)) || !!question?.text.trim() || commentDraft !== null;
  const file = view.files.find((f) => f.diff.path === view.selected_path);
  // Selections are keyed by line numbers, so they only mean anything against
  // the patch they were made on: a refresh that shifts lines drops them.
  useEffect(() => { setSelection(null); setNotice(null); }, [view.workflow_id, view.selected_path, file?.diff.patch]);
  useEffect(() => {
    setQuestion((draft) => {
      const turn = draft?.turn !== undefined ? ai.questions[draft.turn] : null;
      return turn?.answer && turn.question === draft?.text.trim() ? null : draft;
    });
  }, [ai.questions]);
  useEffect(() => {
    const draft = ai.comment_draft;
    const key = `${view.workflow_id}:${draft?.request}`;
    if (draft && receivedDraft.current !== key) {
      receivedDraft.current = key;
      setCommentDraft({ request: draft.request, text: draft.text });
    }
  }, [view.workflow_id, ai.comment_draft]);
  useEffect(() => {
    const ready = ai.ready_comment;
    const key = `${view.workflow_id}:${ready?.request}`;
    if (ready && receivedEditor.current !== key) {
      receivedEditor.current = key;
      setCommentDraft(null);
      if (ready.start && ready.end) setSelection({ anchor: ready.start, cursor: ready.end });
      setEditor({ kind: ready.path ? "line_comment" : "general", path: ready.path ?? "",
        span: ready.start && ready.end ? { start: ready.start, end: ready.end } : undefined,
        text: ready.text, severity: ready.severity, original: ready.original, originalSeverity: ready.severity });
    }
  }, [view.workflow_id, ai.ready_comment]);
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
    if (!file || busy || ai.question_running || dirty || pending) return;
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
    if (commandBusy) return;
    const succeeded = action === "cancel"
      ? (!(ai.ready_comment || ai.comment_draft) || await onAct({ kind: "discard_question_draft" }))
      : await onAct(action);
    if (succeeded) { setEditor(null); setQuestion(null); setCommentDraft(null); setPending(null); }
  }
  function request(action: ReviewAction | "cancel") {
    if (commandBusy) return;
    if (dirty || (action !== "cancel" && ((action.kind === "pause" && ai.running) || action.kind === "apply_suggestion" || action.kind === "discard" || (action.kind === "reload" && view.save_error)))) setPending(action);
    else void run(action);
  }
  function edit(kind: "comment" | "reject" | "general") {
    if (busy || ai.question_running || dirty) return;
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
  const close = () => {
    if (commandBusy) return;
    if (history) void onAct({ kind: "history_close" });
    else if (summary) void onAct({ kind: "summary_close" });
    else request({ kind: "pause" });
  };
  return <Modal label="Final Review" title={`Final Review · ${view.feature_name}`} size="xl" onClose={close}
    footer={<button className="btn btn-secondary" disabled={commandBusy || !!view.finish?.completing} onClick={close}>{history || summary ? "Return to review" : "Pause review"}</button>}>
    <p>Review progress is shared with the TUI. Pause and reopen to resume. Complete the review and hand off feedback from the pre-finish summary.</p>
    <p className="diff-summary">{view.branch} · {view.base_ref} · {approved} approved · {rejected} rejected · {view.files.length - approved - rejected} undecided</p>
    {commandBusy && <p role="status"><Spinner /> Updating review…</p>}
    {(error || view.error) && <p role="alert">{error || view.error}</p>}
    {summary ? <ReviewSummaryPanel view={view} busy={commandBusy || ai.precall !== null} dirty={dirty} onAct={onAct} /> : history ? <section className="review-history" aria-label="Review round history">
      <p className="muted small">Read-only history. Current reflects this open review; local unsaved drafts stay in their editors. Completed rounds include feedback, suggestions, checks and agent replies.</p>
      {history.error && <p role="alert">{history.error}</p>}
      <div className="review-history-reader">
        <nav className="diff-files" aria-label="Review rounds">
          <button className={`diff-file ${history.selected === 0 ? "diff-file-selected" : ""}`} aria-pressed={history.selected === 0} disabled={commandBusy}
            onClick={() => void onAct({ kind: "history_select", round: 0 })}><span>Current</span><small>{history.current_unresolved} open threads</small></button>
          {history.rounds.map((round, index) => <button key={index} className={`diff-file ${history.selected === index + 1 ? "diff-file-selected" : ""}`} aria-pressed={history.selected === index + 1} disabled={commandBusy}
            onClick={() => void onAct({ kind: "history_select", round: index + 1 })}><span>{round.title}</span><small>{round.carried_unresolved} carried unresolved</small></button>)}
          {history.archive_available && !history.archive_loaded && <button className="btn btn-secondary btn-sm" disabled={commandBusy}
            onClick={() => void onAct({ kind: "history_load_older" })}>Load older rounds</button>}
        </nav>
        <section className="review-history-body" key={`${view.workflow_id}:${history.selected}`} aria-label="Review round contents"><Markdown source={history.markdown} /></section>
      </div>
      {history.rounds.length === 0 && <p>No completed rounds loaded.</p>}
    </section> : <>
    <button className="btn btn-secondary btn-sm" disabled={commandBusy || ai.precall !== null || pending !== null}
      onClick={() => void onAct({ kind: "summary_open" })}>Pre-finish summary</button>
    <button className="btn btn-secondary btn-sm" disabled={commandBusy || ai.precall !== null || pending !== null}
      onClick={() => void onAct({ kind: "history_open" })}>Review history</button>
    {view.save_error && <div role="alert"><p>Progress was not saved: {view.save_error}</p><button className="btn btn-secondary" disabled={busy} onClick={() => void onAct({ kind: "retry_save" })}>Retry save</button>
      <button className="btn btn-ghost" disabled={busy || pending !== null} onClick={() => request({ kind: "discard" })}>Close without saving</button></div>}
    {pending && (pending !== "cancel" && pending.kind === "apply_suggestion"
      ? <div className="review-confirm" role="alertdialog" aria-label="Apply suggestion locally">
        <p>Apply the saved replacement to {pending.path}, {locationLabel(pending.start)} – {locationLabel(pending.end)}? This writes to your checkout, resolves the thread, and refreshes the diff. Review the changed code again before approving.</p>
        <button className="btn btn-primary" disabled={busy} onClick={() => void run(pending)}>Apply replacement</button>
        <button className="btn btn-secondary" disabled={busy} onClick={() => setPending(null)}>Cancel application</button>
      </div>
      : pending !== "cancel" && pending.kind === "discard"
      ? <div className="review-confirm" role="alertdialog" aria-label="Close review without saving">
        <p>Close without saving? Review progress since the last successful save is lost. Source changes already applied stay in your checkout.</p>
        <button className="btn btn-danger" disabled={busy} onClick={() => void run(pending)}>Discard and close</button>
        <button className="btn btn-secondary" disabled={busy} onClick={() => setPending(null)}>Keep editing</button>
      </div>
      : <div className="review-confirm" role="alertdialog" aria-label="Discard unsaved review draft">
        <p>{ai.running ? "Discard unsaved edits, cancel the running AI request and continue?" : "Discard unsaved edits and continue?"}</p>
        <button className="btn btn-danger" disabled={commandBusy} onClick={() => void run(pending)}>Discard and continue</button>
        <button className="btn btn-secondary" disabled={commandBusy} onClick={() => setPending(null)}>Keep editing</button>
      </div>)}
    <div className="diff-controls">
      <Field label="Filter review files"><input value={filter} onChange={(event) => setFilter(event.target.value)} /></Field>
      <Field label="Review layout"><select value={split ? "split" : "unified"} onChange={(event) => setSplit(event.target.value === "split")}><option value="unified">Unified</option><option value="split">Side by side</option></select></Field>
      <button className="btn btn-secondary btn-sm" disabled={busy || pending !== null} onClick={() => request({ kind: "refresh" })}>Refresh changes</button>
      <button className="btn btn-ghost btn-sm" disabled={busy || pending !== null} onClick={() => request({ kind: "reload" })}>Reload saved review</button>
      <button className="btn btn-ghost btn-sm" disabled={busy || pending !== null || view.error !== null} onClick={() => request({ kind: "undo" })}>Undo verdict</button>
      <button className="btn btn-secondary btn-sm" disabled={busy || ai.question_running || dirty || pending !== null} onClick={() => edit("general")}>Overall feedback</button>
    </div>
    {ai.precall && <section className="review-confirm" role="alertdialog" aria-label="Review AI call">
      <p>Headless AI call: {ai.precall.title} · {ai.precall.harness}. This reads the checkout and may use paid harness credits.</p>
      {ai.precall.viewing && <pre className="review-note">{ai.precall.preview}</pre>}
      <button className="btn btn-secondary" disabled={commandBusy} onClick={() => void onAct({ kind: "precall_toggle_view" })}>{ai.precall.viewing ? "Hide prompt" : "View prompt"}</button>
      {onEditPrompt && <button className="btn btn-secondary" disabled={commandBusy} onClick={onEditPrompt}>Edit prompt</button>}
      <button className="btn btn-primary" disabled={commandBusy} onClick={() => void onAct({ kind: "precall_confirm" })}>Continue AI call</button>
      <button className="btn btn-secondary" disabled={commandBusy} onClick={() => void onAct({ kind: "precall_cancel" })}>Cancel AI call</button>
    </section>}
    {ai.running && <div role="status"><Spinner /> {ai.question_running ? (ai.comment_draft ? "Checking comment draft context" : "Answering or drafting review question") : ai.co_review_path ? `Co-reviewing ${ai.co_review_path}` : ai.walkthrough_path ? `Generating walkthrough for ${ai.walkthrough_path}` : "Generating changeset overview"}…
      <button className="btn btn-secondary" disabled={commandBusy} onClick={() => void onAct({ kind: "cancel_ai" })}>Cancel AI request</button>
    </div>}
    {ai.message && <p role="status">{ai.message}</p>}
    {ai.question_error && <p role="alert">{ai.question_error}</p>}
    <button className="btn btn-secondary btn-sm" disabled={busy || ai.running || dirty || pending !== null || !view.files.length || view.error !== null}
      onClick={() => void onAct({ kind: "overview" })}>Changeset overview</button>
    <p className="muted small">Walkthroughs, overview and co-review use Claude. Questions offer the project's allowed harnesses.</p>
    {ai.overview && <details className="review-note"><summary>AI changeset overview</summary><Markdown source={ai.overview} /></details>}
    {ai.questions.length > 0 && <details className="review-note"><summary>Review questions ({ai.questions.length})</summary>{ai.questions.map((turn, index) => <section key={index}>
      <p><strong>{turn.question}</strong></p><details><summary>Question context</summary><pre>{turn.focus}</pre></details>
      {turn.answer && <><Markdown source={turn.answer} />
        <button className="btn btn-secondary btn-sm" disabled={busy || ai.running || dirty || editor !== null || question !== null || pending !== null || !ai.harnesses.includes(turn.harness) || !turn.start || !turn.end}
          onClick={() => void onAct({ kind: "draft_question", turn: index, destination: "inline" })}>Draft inline comment</button>
        <button className="btn btn-secondary btn-sm" disabled={busy || ai.running || dirty || editor !== null || question !== null || pending !== null || !ai.harnesses.includes(turn.harness)}
          onClick={() => void onAct({ kind: "draft_question", turn: index, destination: "general" })}>Draft overall feedback</button>
      </>}{turn.error && <p role="alert">{turn.error}</p>}
      {turn.error && turn.path && <button className="btn btn-secondary btn-sm" disabled={busy || ai.running || dirty || pending !== null || !ai.harnesses.length}
        onClick={() => setQuestion({ path: turn.path!, span: turn.start && turn.end ? { start: turn.start, end: turn.end } : null, text: turn.question,
          harness: ai.harnesses.includes(turn.harness) ? turn.harness : ai.harnesses[0] })}>Retry question</button>}
    </section>)}</details>}
    {commentDraft && <form className="review-editor" onSubmit={(event) => {
      event.preventDefault();
      if (!busy && !ai.running && !editor && !question && commentDraft.text.trim()) void onAct({ kind: "transfer_question_draft", request: commentDraft.request, text: commentDraft.text });
    }}>
      <p>AI draft for {ai.comment_draft?.destination === "inline" ? "the question's original line or range" : "overall feedback"}. Review and edit it before opening the comment editor.</p>
      <Field label="AI comment draft"><textarea rows={5} value={commentDraft.text} disabled={busy || ai.running} onChange={(event) => setCommentDraft({ ...commentDraft, text: event.target.value })} /></Field>
      <button className="btn btn-primary" disabled={busy || ai.running || pending !== null || editor !== null || question !== null || !commentDraft.text.trim()}>Open comment editor</button>
      <button type="button" className="btn btn-secondary" disabled={busy || ai.running} onClick={() => request({ kind: "discard_question_draft" })}>Discard AI draft</button>
      <p className="muted small">Opening the editor checks the repository again, runs no AI call and appends to existing feedback. Save the comment explicitly to keep it.</p>
    </form>}
    {question && <form className="review-editor" onSubmit={(event) => {
      event.preventDefault();
      if (!busy && !ai.running && question.text.trim()) {
        setQuestion({ ...question, turn: ai.questions.length });
        void onAct({ kind: "ask", path: question.path, start: question.span?.start ?? null, end: question.span?.end ?? null, question: question.text, harness: question.harness });
      }
    }}>
      <p>Question about {question.path}{question.span && `, ${locationLabel(question.span.start)} – ${locationLabel(question.span.end)}`}</p>
      <p className="muted small">The answering harness reads the repository and reviewed diff. Answers stay in this review while it is open. Follow-ups include earlier answers for this review version.</p>
      <Field label="Review question"><textarea rows={4} value={question.text} disabled={busy || ai.running} onChange={(event) => setQuestion({ ...question, text: event.target.value })} /></Field>
      <Field label="Answering harness"><select value={question.harness} disabled={busy || ai.running} onChange={(event) => setQuestion({ ...question, harness: event.target.value as AgentSlug })}>
        {ai.harnesses.map((harness) => <option key={harness} value={harness}>{harness}</option>)}
      </select></Field>
      <button className="btn btn-primary" disabled={busy || ai.running || pending !== null || !question.text.trim()}>Ask review question</button>
      <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => request("cancel")}>Cancel question</button>
    </form>}
    {view.general_feedback && <section aria-label="Saved overall feedback"><Markdown source={view.general_feedback} /></section>}
    {view.applied_suggestions.length > 0 && <details className="review-note"><summary>Applied locally ({view.applied_suggestions.length})</summary>
      <ul>{view.applied_suggestions.map((anchor, index) => <li key={index}>{anchor}</li>)}</ul>
      <p className="muted small">Closing without saving does not undo source changes already applied.</p>
    </details>}
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
            <button className="btn btn-secondary btn-sm" disabled={busy || ai.question_running || dirty || pending !== null} onClick={() => edit("reject")}>Reject file</button>
            <button className="btn btn-ghost btn-sm" disabled={busy || ai.question_running || dirty || pending !== null} onClick={() => edit("comment")}>Edit file comment</button>
          </div>
          <div className="review-line-controls">
            <button className="btn btn-secondary btn-sm" disabled={busy || ai.running || dirty || pending !== null || file.diff.is_binary || !!file.notes || !!file.walkthrough}
              onClick={() => void onAct({ kind: "walkthrough", path: file.diff.path })}>Generate walkthrough</button>
            <button className="btn btn-secondary btn-sm" disabled={busy || ai.running || dirty || pending !== null || file.diff.is_binary || !file.diff.hunks.length}
              onClick={() => void onAct({ kind: "co_review", path: file.diff.path })}>AI co-review file</button>
            <button className="btn btn-secondary btn-sm" disabled={busy || ai.running || dirty || pending !== null || !ai.harnesses.length}
              onClick={() => { setEditor(null); setQuestion({ path: file.diff.path, span, text: "", harness: ai.harnesses[0] }); }}>Ask about {span ? "selection" : "file"}</button>
          </div>
          {file.walkthrough && <details className="review-note"><summary>AI walkthrough</summary><Markdown source={file.walkthrough} /></details>}
          {file.feedback && <p className="review-note">[{file.severity}] {file.feedback}</p>}
          {file.notes && <details className="review-note"><summary>Developer notes</summary><Markdown source={file.notes} /></details>}
          {file.comment && <div className="review-note"><p>[{file.comment.severity}] {file.comment.text}{file.comment.resolved && " (resolved)"}{file.comment.carried && " (previous round)"}</p>
            <button className="btn btn-ghost btn-sm" disabled={busy || pending !== null} onClick={() => request({ kind: "toggle_resolved", path: file.diff.path })}>{file.comment.resolved ? "Reopen comment" : "Resolve comment"}</button>
          </div>}
          {file.line_comments.length > 0 && <details className="review-note"><summary>Saved line comments ({file.line_comments.length})</summary>
            {file.line_comments.map((comment, index) => <div key={index}><p>{comment.anchor} [{comment.severity}] {comment.text}{comment.resolved && " (resolved)"}{comment.draft && " (AI draft)"}{comment.anchor_lost && " (anchor lost)"}</p>{comment.suggestion !== null && <pre>{comment.suggestion}</pre>}
              {comment.draft && <>
                <button className="btn btn-secondary btn-sm" disabled={busy || dirty || pending !== null || !comment.editable} onClick={() => request({ kind: "accept_draft", path: file.diff.path, start: comment.start, end: comment.end })}>Accept AI draft</button>
                <button className="btn btn-ghost btn-sm" disabled={busy || dirty || pending !== null || !comment.editable} onClick={() => request({ kind: "dismiss_draft", path: file.diff.path, start: comment.start, end: comment.end })}>Dismiss AI draft</button>
              </>}
              <button className="btn btn-ghost btn-sm" disabled={busy || ai.question_running || dirty || pending !== null || !comment.editable} onClick={() => editSpan("line_comment", comment)}>Edit line comment</button>
              <button className="btn btn-ghost btn-sm" disabled={busy || ai.question_running || dirty || pending !== null || !comment.editable} onClick={() => editSpan("suggestion", comment)}>Edit suggestion</button>
              {comment.suggestion !== null && <>
                <button className="btn btn-secondary btn-sm" disabled={busy || dirty || pending !== null || view.save_error !== null || comment.apply_blocked !== null}
                  onClick={() => request({ kind: "apply_suggestion", path: file.diff.path, start: comment.start, end: comment.end })}>Apply suggestion locally</button>
                {comment.apply_blocked && <p className="muted small">Cannot apply locally: {comment.apply_blocked}</p>}
              </>}
              <button className="btn btn-ghost btn-sm" disabled={busy || pending !== null || !comment.editable || comment.draft} onClick={() => request({ kind: "toggle_line_resolved", path: file.diff.path, start: comment.start, end: comment.end })}>{comment.resolved ? "Reopen thread" : "Resolve thread"}</button>
            </div>)}
          </details>}
          {!file.diff.is_binary && lines.length > 0 && <div className="review-line-controls">
            <p className="muted small">Click a line number to select it; Shift-click another to select a range in diff order.</p>
            {span && <p>Selected {locationLabel(span.start)} – {locationLabel(span.end)}</p>}
            {notice && <p role="alert">{notice}</p>}
            <button className="btn btn-secondary btn-sm" disabled={busy || ai.question_running || dirty || pending !== null || !span} onClick={() => span && editSpan("line_comment", span)}>Comment on selection</button>
            <button className="btn btn-secondary btn-sm" disabled={busy || ai.question_running || dirty || pending !== null || !span} onClick={() => span && editSpan("suggestion", span)}>Suggest replacement</button>
          </div>}
          <div className="diff-code">
            {file.diff.is_binary ? <p>Binary file changed; no text diff is available.</p> : file.diff.hunks.length === 0 ? <pre>{file.diff.patch || "No textual changes."}</pre>
              : file.diff.hunks.map((hunk, index) => <Hunk key={index} hunk={hunk} split={split} selection={{ disabled: busy || dirty || pending !== null, select: selectLine,
                contains: (line) => !!span && line.kind !== "marker" && lineIndex(line) >= lineIndex(span.start) && lineIndex(line) <= lineIndex(span.end) }} />)}
          </div>
        </> : <p>{view.files.length ? "Select a file to review." : "No changes to review."}</p>}
      </section>
    </div>}
    </>}
  </Modal>;
}
