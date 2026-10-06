import { useEffect, useRef, useState } from "react";
import type { ReviewAction, ReviewView } from "./api";

export default function ReviewSummaryPanel({ view, busy, dirty, onAct }: {
  view: ReviewView; busy: boolean; dirty: boolean; onAct: (action: ReviewAction) => Promise<boolean>;
}) {
  const summary = view.summary!;
  const [confirm, setConfirm] = useState(false);
  const [checkConfirm, setCheckConfirm] = useState<string | null>(null);
  const [completeConfirm, setCompleteConfirm] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const inFlight = useRef(false);
  useEffect(() => { setConfirm(false); setCheckConfirm(null); setCompleteConfirm(false); }, [view.workflow_id, view.revision, view.check_command]);
  const checkRunning = view.check?.status === "running";
  const finish = view.finish;
  const completing = finish?.completing ?? false;
  const generated = view.ai.comment_draft !== null || view.ai.ready_comment !== null;
  const blocked = busy || submitting || dirty || checkRunning || view.ai.running || view.save_error !== null || view.error !== null;
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
  async function runCheck() {
    if (blocked || inFlight.current || !checkConfirm) return;
    inFlight.current = true;
    setSubmitting(true);
    try {
      if (await onAct({ kind: "run_check", command: checkConfirm })) setCheckConfirm(null);
    } finally {
      inFlight.current = false;
      setSubmitting(false);
    }
  }
  async function complete(deliver: boolean) {
    if (!finish || blocked || generated || inFlight.current) return;
    inFlight.current = true;
    setSubmitting(true);
    try {
      // Every expectation this confirmation showed is checked again before
      // anything is written, so a changed check, batch or target is refused.
      if (await onAct({ kind: "complete", check_command: view.check_command, apply_suggestions: finish.apply_suggestions,
        handoff_session: finish.handoff?.session_id ?? null, deliver })) setCompleteConfirm(false);
    } finally {
      inFlight.current = false;
      setSubmitting(false);
    }
  }
  const target = finish?.handoff;
  const delivery = target && (finish.submit_prompt && !target.stopped
    ? `sends the "address the feedback" prompt to ${target.label}`
    : `opens the "address the feedback" prompt as an unsent draft in ${target.label}${finish.submit_prompt ? " (stopped, so it cannot be sent yet)" : ""}`);
  return <section className="review-summary" aria-label="Pre-finish review summary">
    <p>Every file verdict and open kept thread is shown below, including files hidden by your filter. Run your configured project check here, then complete the review and choose whether to hand feedback to an agent.</p>
    {summary.undecided > 0 && <p role="status">{summary.undecided} file(s) have no verdict. Return to review to decide them before finishing.</p>}
    {dirty && <p role="status">Your unsaved drafts are retained. Return to review and save or discard them before applying the batch or running checks.</p>}
    {view.save_error && <div role="alert"><p>Progress was not saved: {view.save_error}</p>
      <button className="btn btn-secondary" disabled={busy || submitting} onClick={() => void onAct({ kind: "retry_save" })}>Retry save</button>
    </div>}
    {summary.failures.length > 0 && <div role="alert"><p>Some suggestions could not be applied. They remain open for editing or feedback.</p>
      <ul>{summary.failures.map((failure, index) => <li key={index}>{failure}</li>)}</ul>
    </div>}
    <p>{summary.pending_suggestions} open suggestion(s) · {view.applied_suggestions.length} applied locally</p>
    <button className="btn btn-primary" disabled={blocked || summary.pending_suggestions === 0 || confirm || checkConfirm !== null} onClick={() => setConfirm(true)}>Apply pending suggestions</button>
    {confirm && <div className="review-confirm" role="alertdialog" aria-label="Apply pending suggestions locally">
      <p>Apply {summary.pending_suggestions} saved replacement(s) to your checkout? This writes source files, resolves successful threads, and refreshes the diff. Blocked suggestions stay open. Changed files lose approval and must be reviewed again. This does not finish the review or send feedback.</p>
      <button className="btn btn-primary" disabled={blocked} onClick={() => void apply()}>Apply batch locally</button>
      <button className="btn btn-secondary" disabled={busy || submitting} onClick={() => setConfirm(false)}>Cancel batch application</button>
    </div>}
    <section aria-label="Project review check" className="review-note">
      <h3>Project check</h3>
      {view.check_command ? <><pre>{view.check_command}</pre>
        <button className="btn btn-secondary" disabled={blocked || checkConfirm !== null || confirm} onClick={() => setCheckConfirm(view.check_command)}>Run project check</button>
      </> : <p>No final review check is configured for this project.</p>}
      {checkConfirm !== null && <div className="review-confirm" role="alertdialog" aria-label="Run project review check">
        <p>Run this shell command in the feature checkout? It may write build or test artifacts. Review progress stays open; this does not apply suggestions, finish the review or send feedback.</p>
        <pre>{checkConfirm}</pre>
        <button className="btn btn-primary" disabled={blocked} onClick={() => void runCheck()}>Run check now</button>
        <button className="btn btn-secondary" disabled={busy || submitting} onClick={() => setCheckConfirm(null)}>Cancel check launch</button>
      </div>}
      {view.check && <><p role="status">Check {view.check.status}: {view.check.command}</p>
        {view.check.output && <pre aria-label="Project check output">{view.check.output}</pre>}
        {checkRunning && <button className="btn btn-secondary" disabled={busy || submitting} onClick={() => void onAct({ kind: "cancel_check" })}>Cancel running check</button>}
        <p className="muted small">{completing ? "Completion waits for this check; its result is recorded in the round." : "Results stay in this open review. Completing the review runs the configured check again."}</p>
      </>}
    </section>
    {finish && <section aria-label="Complete Final Review" className="review-note">
      <h3>Complete review</h3>
      <p>{finish.approved} approved · {finish.needs_work} need work · {finish.skipped} skipped · {finish.file_comments} file comment(s) · {finish.line_comments} line comment(s){finish.general_feedback ? " · overall feedback" : ""}</p>
      {generated && <p role="status">Transfer or discard the generated comment draft before completing.</p>}
      {completing
        ? <><p role="status">Completing: the configured check is running. The feedback round is recorded when it finishes. Any suggestions applied before the check remain in source files if you cancel.</p>
          <button className="btn btn-secondary" disabled={busy || submitting} onClick={() => void onAct({ kind: "cancel_check" })}>Cancel completion</button></>
        : <button className="btn btn-primary" disabled={blocked || generated || confirm || checkConfirm !== null || completeConfirm} onClick={() => setCompleteConfirm(true)}>Complete review…</button>}
      {completeConfirm && !completing && <div className="review-confirm" role="alertdialog" aria-label="Complete Final Review">
        <p>Record this round in .claude/final-review-feedback.md, clear the saved progress and close the review? The round is shared with the TUI's review history.</p>
        <ul>
          {finish.skipped > 0 && <li>{finish.skipped} file(s) have no verdict and are recorded as skipped.</li>}
          {finish.apply_suggestions > 0 && <li>This review is set to apply suggestions on finish: {finish.apply_suggestions} saved replacement(s) are written to source before the check. Cancelling completion during the check leaves those source changes in place.</li>}
          {view.check_command
            ? <li>Runs the configured check first, as the terminal review does: <code>{view.check_command}</code>. Pass or fail, its result is recorded; earlier results shown here are not reused.</li>
            : <li>No project check is configured.</li>}
          {finish.post_to_pr && <li>Also posts the feedback to this branch's GitHub pull request (configured).</li>}
          <li>{target
            ? `Handing off ${delivery} when the round has actionable feedback: rejections, open comments, overall feedback or a failed check.`
            : "This feature has no agent session; the feedback is saved for later."}</li>
        </ul>
        {target && <button className="btn btn-primary" disabled={blocked || generated} onClick={() => void complete(true)}>Complete and hand off to {target.label}</button>}
        <button className={target ? "btn btn-secondary" : "btn btn-primary"} disabled={blocked || generated} onClick={() => void complete(false)}>{target ? "Complete without handoff" : "Complete review"}</button>
        <button className="btn btn-secondary" disabled={busy || submitting} onClick={() => setCompleteConfirm(false)}>Keep reviewing</button>
      </div>}
    </section>}
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
