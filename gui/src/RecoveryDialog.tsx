import type { SavedAgentSession, SessionRecoveryChoice, SessionRecoveryOption } from "./api";
import { Modal, Spinner } from "./ui";

export default function RecoveryDialog({
  option,
  sessions,
  selectedId,
  loading,
  busy,
  onChoose,
  onBack,
  onSelect,
  onRecover,
  onClose,
}: {
  option: SessionRecoveryOption;
  sessions: SavedAgentSession[] | null;
  selectedId: string | null;
  loading: boolean;
  busy: boolean;
  onChoose: () => void;
  onBack: () => void;
  onSelect: (id: string) => void;
  onRecover: (choice: SessionRecoveryChoice, pickedId: string | null) => void;
  onClose: () => void;
}) {
  return (
    <Modal
      label="Recover agent session"
      title={`Recover ${option.harness} session`}
      subtitle="This agent is not running, but AMF still has its saved session."
      onClose={onClose}
      dismissable={!busy}
      footer={sessions === null ? (
        <>
          <button className="btn btn-ghost" onClick={onClose}>Cancel</button>
          <button className="btn btn-secondary" disabled={busy} onClick={() => onRecover("clear", null)}>Start a new conversation</button>
          <button className="btn btn-primary" disabled={busy} onClick={() => onRecover("resume", null)}>
            {busy && <Spinner />}Continue where it left off
          </button>
        </>
      ) : (
        <>
          <button className="btn btn-ghost" onClick={onBack}>Back</button>
          <button className="btn btn-primary" disabled={!selectedId || busy} onClick={() => onRecover("pick", selectedId)}>
            {busy && <Spinner />}Continue this conversation
          </button>
        </>
      )}
    >
      {sessions === null ? (
        <>
          <ul className="recovery-choices">
            <li><strong>Continue where it left off</strong>: resume the last conversation.</li>
            <li><strong>Start a new conversation</strong>: the old one stays saved.</li>
            <li><strong>Pick an older conversation</strong>: choose from this feature's saved ones.</li>
          </ul>
          <button className="btn btn-secondary recovery-choose" disabled={loading} onClick={onChoose}>
            {loading && <Spinner />}Pick an older conversation
          </button>
        </>
      ) : sessions.length === 0 ? (
        <p className="muted">No saved conversations were found for this feature.</p>
      ) : (
        <div className="recovery-session-list">
          {sessions.map((session) => (
            <label key={session.id} className="recovery-session-row">
              <input type="radio" name="recovery-session" checked={selectedId === session.id}
                onChange={() => onSelect(session.id)} />
              <span title={session.id}><strong>{session.title}</strong></span>
            </label>
          ))}
        </div>
      )}
    </Modal>
  );
}
