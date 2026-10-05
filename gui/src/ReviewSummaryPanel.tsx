import { useEffect, useRef, useState } from "react";
import type { ReviewAction, ReviewView } from "./api";

export default function ReviewSummaryPanel({ view, busy, dirty, onAct }: {
  view: ReviewView; busy: boolean; dirty: boolean; onAct: (action: ReviewAction) => Promise<boolean>;
}) {
  const summary = view.summary!;
  const [confirm, setConfirm] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const inFlight = useRef(false);
  useEffect(() => { setConfirm(false); }, [view.workflow_id, view.revision]);
  const blocked = busy || submitting || dirty || view.ai.running || view.save_error !== null || view.error !== null;
  async function apply() {
    if (blocked || inFlight.current) return;
    inFlight.current = true;
    setSubmitting(true);
    try {
      if (await onAct({ kind: "apply_finish_suggestions" })) setConfirm(false);
    } finally {
      inFlight.current = false;
      setSubmitting(false);
    }
  }
  return <section className="review-summary" aria-label="Pre-finish review summary">
    <p>Every file verdict and open kept thread is shown below, including files hidden by your filter. Finishing, checks and feedback handoff still use the TUI.</p>
    {summary.undecided > 0 && <p role="status">{summary.undecided} file(s) have no verdict. Return to review to decide them before finishing.</p>}
    {dirty && <p role="status">Your unsaved drafts are retained. Return to review and save or discard them before applying the batch.</p>}
    {view.save_error && <div role="alert"><p>Progress was not saved: {view.save_error}</p>
      <button className="btn btn-secondary" disabled={busy || submitting} onClick={() => void onAct({ kind: "retry_save" })}>Retry save</button>
    </div>}
    {summary.failures.length > 0 && <div role="alert"><p>Some suggestions could not be applied. They remain open for editing or feedback.</p>
      <ul>{summary.failures.map((failure, index) => <li key={index}>{failure}</li>)}</ul>
    </div>}
    <p>{summary.pending_suggestions} open suggestion(s) · {view.applied_suggestions.length} applied locally</p>
    <button className="btn btn-primary" disabled={blocked || summary.pending_suggestions === 0 || confirm} onClick={() => setConfirm(true)}>Apply pending suggestions</button>
    {confirm && <div className="review-confirm" role="alertdialog" aria-label="Apply pending suggestions locally">
      <p>Apply {summary.pending_suggestions} saved replacement(s) to your checkout? This writes source files, resolves successful threads, and refreshes the diff. Blocked suggestions stay open. Changed files lose approval and must be reviewed again. This does not finish the review or send feedback.</p>
      <button className="btn btn-primary" disabled={blocked} onClick={() => void apply()}>Apply batch locally</button>
      <button className="btn btn-secondary" disabled={busy || submitting} onClick={() => setConfirm(false)}>Cancel batch application</button>
    </div>}
    {summary.rows.map((row, index) => <article className="review-note" key={index}>
      <h3>{row.title}</h3>
      {row.severity && <p className="muted small">{row.severity}</p>}
      {row.text && <pre className="review-note">{row.text}</pre>}
      {row.suggestion !== null && <><p>Suggested replacement</p><pre>{row.suggestion}</pre></>}
      {row.suggestion !== null && row.apply_blocked && <p className="muted small">Cannot apply locally: {row.apply_blocked}</p>}
    </article>)}
    {view.applied_suggestions.length > 0 && <details className="review-note"><summary>Applied locally ({view.applied_suggestions.length})</summary>
      <ul>{view.applied_suggestions.map((anchor, index) => <li key={index}>{anchor}</li>)}</ul>
    </details>}
  </section>;
}
