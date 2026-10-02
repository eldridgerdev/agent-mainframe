import { useState } from "react";
import { AgentSlug, LearningAction, LearningView } from "./api";
import Markdown from "./Markdown";
import { Field, Icon, Modal, Spinner } from "./ui";

/** The question draft stays local across backend polls and navigation. */
export default function LearningPanel({ view, busy, onAct, onLaunch, onClose }: {
  view: LearningView;
  busy: boolean;
  onAct: (action: LearningAction) => Promise<boolean>;
  onLaunch: (qaId: string) => void;
  onClose: () => void;
}) {
  const [question, setQuestion] = useState("");
  const [intent, setIntent] = useState("explain");
  const [parentId, setParentId] = useState<string | null>(null);
  const [selectedQa, setSelectedQa] = useState<string | null>(null);
  const [confirmClose, setConfirmClose] = useState(false);
  const [rangeStart, setRangeStart] = useState<number | null>(null);
  const parent = view.qa.find((qa) => qa.id === parentId);
  const answer = view.qa.find((qa) => qa.id === selectedQa) ?? view.qa[view.qa.length - 1];
  async function ask() {
    if (await onAct({ kind: "ask", question, intent, parent_id: parentId })) {
      setQuestion(""); setParentId(null); setSelectedQa(null);
    }
  }
  return (
    <Modal label="Learning" title={`Learning · ${view.feature_name}`} size="lg"
      subtitle="Read code and ask questions. Opening an editing agent is a separate action."
      dismissable={!busy} onClose={() => question.trim() ? setConfirmClose(true) : onClose()}>
      {confirmClose && <div className="callout callout-warning" role="alert">
        <p>Discard this unsent question and close Learning?</p>
        <button className="btn btn-secondary" onClick={() => setConfirmClose(false)}>Keep writing</button>
        <button className="btn btn-warning" disabled={busy} onClick={onClose}>Discard question and close</button>
      </div>}
      <div className="learning-controls">
        <button className="btn btn-secondary btn-sm" disabled={busy || !view.is_git}
          onClick={() => { setRangeStart(null); void onAct({ kind: "toggle_scope" }); }}>
          {view.scope === "repo_tree" ? "Show branch changes" : "Show repo tree"}
        </button>
        <button className="btn btn-ghost btn-sm" disabled={busy} onClick={() => void onAct({ kind: "refresh" })}>Refresh files</button>
        <Field label="Answering harness">
          <select value={view.harness} disabled={busy} onChange={(e) => void onAct({ kind: "settings", harness: e.target.value as AgentSlug, level: view.level })}>
            {view.harnesses.map((h) => <option key={h} value={h}>{h}</option>)}
          </select>
        </Field>
        <Field label="Reading level">
          <select value={view.level} disabled={busy} onChange={(e) => void onAct({ kind: "settings", harness: view.harness, level: e.target.value })}>
            <option value="newcomer">Newcomer</option><option value="familiar">Familiar</option>
          </select>
        </Field>
      </div>
      {!view.history_saved && <p role="status">History is in memory only; it will not survive closing AMF.</p>}
      {view.error && <p role="alert">{view.error}</p>}
      {view.notice && <p role="status">{view.notice}</p>}
      <div className="learning-reader">
        <nav className="learning-files" aria-label="Learning files">
          {view.entries.map((entry) => (
            <button key={entry.key} className="learning-file" disabled={busy}
              style={{ paddingLeft: 8 + entry.depth * 14 }}
              onClick={() => { setRangeStart(null); void onAct({ kind: "select_entry", key: entry.key }); }}>
              <Icon name={entry.kind === "dir" ? (entry.expanded ? "chevronDown" : "chevronRight") : "file"} size={12} />
              {entry.label}
            </button>
          ))}
        </nav>
        <section className="learning-content" aria-label="Code reader">
          <div className="learning-code-header">
            <strong>{view.content_path ?? "This project"}</strong>
            <button className="btn btn-ghost btn-sm" disabled={busy || !view.content_path}
              onClick={() => { setRangeStart(null); void onAct({ kind: "file_anchor" }); }}>Ask about file</button>
            <button className="btn btn-ghost btn-sm" disabled={busy}
              onClick={() => { setRangeStart(null); void onAct({ kind: "project_anchor" }); }}>Ask about project</button>
          </div>
          {view.content_error ? <p role="alert">{view.content_error}</p> : (
            <div className="learning-code">
              {view.content.map((line, index) => (
                <button key={index} className={`learning-line${rangeStart === index + 1 ? " learning-line-selected" : ""}`}
                  disabled={busy} aria-label={`Select line ${index + 1}`}
                  onClick={(e) => {
                    const line = index + 1;
                    const start = e.shiftKey && rangeStart !== null ? Math.min(rangeStart, line) : line;
                    const end = e.shiftKey && rangeStart !== null ? Math.max(rangeStart, line) : line;
                    if (!e.shiftKey) setRangeStart(line);
                    void onAct({ kind: "lines_anchor", start, end });
                  }}>
                  <span className="learning-line-number">{view.content_line_labels[index]}</span><code>{line || " "}</code>
                </button>
              ))}
            </div>
          )}
        </section>
      </div>
      <p className="field-hint">Click a line, then Shift-click to select a range. Next question: {parent ? parent.anchor : view.anchor}.</p>
      <form className="learning-question" onSubmit={(e) => { e.preventDefault(); void ask(); }}>
        {parent && <p>Follow-up to: {parent.question} <button type="button" className="link-button" disabled={busy} onClick={() => setParentId(null)}>Cancel follow-up</button></p>}
        <Field label="Question"><textarea value={question} onChange={(e) => setQuestion(e.target.value)} rows={3} /></Field>
        <div className="learning-controls">
          <Field label="Question intent"><select value={intent} onChange={(e) => setIntent(e.target.value)}>
            <option value="explain">Explain this to me</option><option value="action">Propose a change</option>
          </select></Field>
          <button className="btn btn-primary" disabled={busy || !question.trim()}>{busy && <Spinner />} Ask {view.harness}</button>
          <span className="field-hint">{view.harness === "codex" ? "Codex reads the repository in a read-only sandbox." : "Answers use the selected code. Deep dive lets the agent read the repository."}</span>
        </div>
      </form>
      <div className="learning-history">
        <nav aria-label="Question history">
          {view.qa.map((qa) => <button key={qa.id} className="learning-file" onClick={() => setSelectedQa(qa.id)}>
            {qa.parent_id && "↳ "}{qa.question} <span className="muted">{qa.status}</span>
          </button>)}
        </nav>
        {answer && <section aria-label="Learning answer">
          <h3>{answer.question}</h3><p className="field-hint">{answer.anchor} · {answer.harness} · {answer.run_mode} · {answer.status}</p>
          {answer.drift && <p role="status">{answer.drift}</p>}
          {answer.error && <p role="alert">{answer.error}</p>}
          {answer.answer && <Markdown source={answer.answer} />}
          <div className="learning-controls">
            <button className="btn btn-secondary btn-sm" disabled={busy || !answer.answer} onClick={() => { setParentId(answer.id); setIntent(answer.intent); }}>Follow up</button>
            <button className="btn btn-secondary btn-sm" disabled={busy || answer.run_mode === "read the repo" || answer.status === "running" || answer.status === "pending"}
              onClick={() => void onAct({ kind: "deep_dive", qa_id: answer.id })}>Deep dive</button>
            <button className="btn btn-warning btn-sm" disabled={busy || !answer.answer || !!question.trim()} title={question.trim() ? "Send or clear your question before opening an editing agent" : undefined} onClick={() => onLaunch(answer.id)}>Open editing agent</button>
          </div>
        </section>}
      </div>
    </Modal>
  );
}
