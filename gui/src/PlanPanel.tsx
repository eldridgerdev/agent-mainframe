import { ReactNode, useEffect, useState } from "react";
import { PlanAction, PlanInput, PlanView, PrecallView } from "./api";
import Markdown from "./Markdown";
import { Icon, IconName, Spinner } from "./ui";

const LOADING_TEXT: Record<string, string> = {
  ai_loading: "Generating follow-up questions…",
  synthesis_loading: "Drafting the plan…",
  directed_feedback_loading: "Revising the plan from your feedback…",
  investigation_loading: "Investigating and revising…",
  critique_loading: "Reviewing the plan…",
  done: "Finishing up…",
};

const PHASE_TITLE: Record<string, string> = {
  resume_prompt: "Resume",
  brief: "Brief",
  static_questions: "Questions",
  ai_consent: "Follow-ups",
  review: "Review",
  editing: "Edit plan",
  directed_feedback: "Feedback",
  investigation: "Investigate",
  critique: "AI review",
  kickoff_handoff: "Kickoff",
};

export default function PlanPanel({
  view,
  precall,
  busy,
  onAct,
}: {
  view: PlanView;
  precall: PrecallView | null;
  busy: boolean;
  onAct: (action: PlanAction, input?: PlanInput) => Promise<void>;
}) {
  const [text, setText] = useState(view.editor_text);
  const [selectedOption, setSelectedOption] = useState<number | null>(view.selected_option);
  const [docPath, setDocPath] = useState("");
  const [confirmCancel, setConfirmCancel] = useState(false);

  useEffect(() => {
    setText(view.editor_text);
    setSelectedOption(view.selected_option);
  }, [view.step_key, view.editor_text, view.selected_option]);

  useEffect(() => setConfirmCancel(false), [view.step_key]);

  const input = { text, selected_option: selectedOption };
  const edit = (rows = 8, placeholder?: string) => (
    <textarea
      aria-label="Plan answer"
      className="plan-textarea"
      value={text}
      placeholder={placeholder}
      onChange={(event) => setText(event.target.value)}
      rows={rows}
      autoFocus
    />
  );
  const button = (
    label: string,
    action: PlanAction,
    value?: PlanInput,
    variant: "primary" | "secondary" | "ghost" | "danger" = "secondary",
    icon?: IconName,
  ) => (
    <button
      key={action}
      className={`btn btn-${variant}`}
      disabled={busy}
      onClick={() => void onAct(action, value)}
    >
      {icon && <Icon name={icon} />}
      {label}
    </button>
  );
  const loading = LOADING_TEXT[view.phase];

  if (precall) {
    return (
      <section className="plan">
        <div role="dialog" aria-label="Approve planning agent call" className="precall">
          <div className="callout callout-accent">
            <Icon name="sparkles" />
            <div>
              <strong>{precall.title}</strong>
              <p>This step calls {precall.harness}. Review the prompt if you like, then continue.</p>
            </div>
          </div>
          {precall.viewing && <pre className="doc doc-mono">{precall.preview}</pre>}
          <Actions
            left={
              <button className="btn btn-ghost" disabled={busy} onClick={() => void onAct("precall_toggle_view")}>
                <Icon name="file" />
                {precall.viewing ? "Hide prompt" : "View prompt"}
              </button>
            }
          >
            {button("Cancel call", "precall_cancel", undefined, "ghost")}
            {button("Continue", "precall_confirm", undefined, "primary")}
          </Actions>
        </div>
      </section>
    );
  }

  return (
    <section className="plan">
      {PHASE_TITLE[view.phase] && (
        <div className="plan-phase">
          <span className="tag tag-accent">{PHASE_TITLE[view.phase]}</span>
          {view.phase === "static_questions" && (
            <span className="muted small">
              Question {view.question_index + 1} of {view.question_count}
            </span>
          )}
          {view.phase === "static_questions" && view.question_count > 0 && (
            <div className="progress" aria-hidden="true">
              <div style={{ width: `${((view.question_index + 1) / view.question_count) * 100}%` }} />
            </div>
          )}
        </div>
      )}

      {loading && (
        <div role="status" className="plan-loading">
          <Spinner />
          <div>
            <strong>{loading}</strong>
            <p className="muted small">The agent is working. You can minimize this window; it will update when it finishes.</p>
          </div>
        </div>
      )}

      {view.phase === "resume_prompt" && (
        <>
          <p className="plan-lead">A saved draft is available for this feature.</p>
          <Actions>
            {button("Discard draft", "discard_draft", undefined, "ghost")}
            {button("Resume draft", "resume", undefined, "primary")}
          </Actions>
        </>
      )}

      {view.phase === "brief" && (
        <>
          <p className="plan-lead">Describe the change you want to plan.</p>
          {edit(8, "What should change, and why?")}
          <div className="attach">
            <div className="attach-row">
              <Icon name="file" />
              <input
                value={docPath}
                onChange={(event) => setDocPath(event.target.value)}
                placeholder="Attach a reference document (path)"
                aria-label="Reference document path"
              />
              <button
                className="btn btn-sm btn-secondary"
                disabled={busy || !docPath.trim()}
                onClick={() => {
                  void onAct("attach_doc", { text: docPath.trim(), selected_option: null })
                    .then(() => setDocPath(""));
                }}
              >Attach</button>
            </div>
            {view.attached_docs.length > 0 && (
              <div className="attach-list">
                {view.attached_docs.map((path) => (
                  <span key={path} className="chip mono" title={path}>{path}</span>
                ))}
                <button className="btn btn-sm btn-ghost" disabled={busy} onClick={() => void onAct("remove_doc")}>
                  Remove last document
                </button>
              </div>
            )}
          </div>
          <Actions>
            {button("Draft plan now", "finish_early", input, "ghost")}
            {button("Continue", "next", input, "primary")}
          </Actions>
        </>
      )}

      {view.phase === "static_questions" && view.question && (
        <>
          <h3 className="plan-question">
            {view.question.text}
            {view.question.optional && <span className="muted small"> (optional)</span>}
          </h3>
          {view.question.options && (
            <div role="radiogroup" aria-label={view.question.text} className="options">
              {view.question.options.map((option, index) => (
                <label key={option} className={selectedOption === index ? "option option-selected" : "option"}>
                  <input
                    type="radio"
                    checked={selectedOption === index}
                    onChange={() => setSelectedOption(index)}
                  />
                  <span>{option}</span>
                </label>
              ))}
              {selectedOption !== null && (
                <button className="btn btn-sm btn-ghost" onClick={() => setSelectedOption(null)}>Clear choice</button>
              )}
            </div>
          )}
          {edit(view.question.options ? 3 : 6, view.question.options ? "Additional context (optional)" : "Your answer")}
          <Actions
            left={
              <>
                {button("Back", "back", input, "ghost")}
                {button("Restore previous answer", "restore_prior", undefined, "ghost")}
              </>
            }
          >
            {button("Draft plan now", "finish_early", input, "ghost")}
            {view.question.optional && button("Skip", "skip", undefined, "secondary")}
            {button("Continue", "next", input, "primary")}
          </Actions>
        </>
      )}

      {view.phase === "ai_consent" && (
        <>
          <p className="plan-lead">The core questions are complete.</p>
          <p className="muted">AI follow-ups and synthesis use agent tokens. Choose how to draft the plan.</p>
          <div className="choice-cards">
            <ChoiceCard icon="sparkles" title="Use AI follow-ups" body="The agent asks a few more targeted questions first." disabled={busy} onClick={() => void onAct("opt_in_ai")} />
            <ChoiceCard icon="zap" title="Synthesize now" body="The agent drafts the plan from your answers." disabled={busy} onClick={() => void onAct("finish_early")} />
            <ChoiceCard icon="file" title="Draft without AI" body="Build the plan straight from your answers." disabled={busy} onClick={() => void onAct("next")} />
          </div>
          <Actions left={button("Back", "back", undefined, "ghost")} />
        </>
      )}

      {view.phase === "review" && (
        <>
          <p className="muted small">Review the plan. Accepting writes the plan file and continues the launch.</p>
          <Markdown source={view.review_markdown ?? ""} />
          <Actions
            left={
              <>
                {button("Edit markdown", "begin_edit", undefined, "ghost")}
                {button("Regenerate", "regenerate", undefined, "ghost")}
                {button("AI review", "request_critique", undefined, "ghost")}
                {button("Investigate", "begin_investigation", undefined, "ghost")}
              </>
            }
          >
            {button("Give feedback", "begin_feedback", undefined, "secondary")}
            {button("Accept plan", "accept", undefined, "primary", "check")}
          </Actions>
        </>
      )}

      {view.phase === "editing" && (
        <>
          <p className="muted small">Edit the proposed markdown. Saving returns to review; the file is written only on acceptance.</p>
          {edit(18)}
          <Actions>
            {button("Discard edit", "cancel_edit", undefined, "ghost")}
            {button("Save edit", "save_edit", input, "primary")}
          </Actions>
        </>
      )}

      {view.phase === "directed_feedback" && (
        <>
          <p className="plan-lead">What should the planning agent change?</p>
          {edit(6)}
          <Actions>
            {button("Back to plan", "cancel_feedback", undefined, "ghost")}
            {button("Revise plan", "submit_feedback", input, "primary")}
          </Actions>
        </>
      )}

      {view.phase === "investigation" && (
        <>
          <p className="plan-lead">What should be investigated?</p>
          <p className="muted small">Blank lines separate independent investigations.</p>
          {edit(6)}
          <Actions>
            {button("Back to plan", "cancel_investigation", undefined, "ghost")}
            {button("Investigate and revise", "submit_investigation", input, "primary")}
          </Actions>
        </>
      )}

      {view.phase === "critique" && (
        <>
          <Markdown source={view.critique ?? ""} />
          <Actions>
            {button("Back to plan", "close_critique", undefined, "ghost")}
            {button("Revise from review", "revise_from_critique", undefined, "primary")}
          </Actions>
        </>
      )}

      {view.phase === "kickoff_handoff" && (
        <>
          <div className="callout callout-success">
            <Icon name="check" />
            <p>Plan saved. Send the kickoff instruction to {view.kickoff_target}?</p>
          </div>
          <Actions>
            {button("Leave session alone", "kickoff_decline", undefined, "ghost")}
            {button("Send kickoff", "kickoff_accept", undefined, "primary", "send")}
          </Actions>
        </>
      )}

      {view.phase !== "kickoff_handoff" && (
        <div className="plan-cancel">
          {confirmCancel ? (
            <div className="callout callout-danger">
              <p>Cancel this interview? The plan will not be accepted or launched.</p>
              <div className="row">
                <button className="btn btn-sm btn-ghost" onClick={() => setConfirmCancel(false)}>Keep planning</button>
                {button("Cancel interview", "cancel", undefined, "danger")}
              </div>
            </div>
          ) : (
            <button className="link-button" onClick={() => setConfirmCancel(true)}>Cancel interview…</button>
          )}
        </div>
      )}
    </section>
  );
}

function Actions({ left, children }: { left?: ReactNode; children?: ReactNode }) {
  return (
    <div className="actions">
      <div className="row">{left}</div>
      <div className="row">{children}</div>
    </div>
  );
}

function ChoiceCard({
  icon,
  title,
  body,
  disabled,
  onClick,
}: {
  icon: IconName;
  title: string;
  body: string;
  disabled: boolean;
  onClick: () => void;
}) {
  return (
    <button className="choice-card" disabled={disabled} onClick={onClick}>
      <Icon name={icon} size={18} />
      <strong>{title}</strong>
      <span>{body}</span>
    </button>
  );
}
