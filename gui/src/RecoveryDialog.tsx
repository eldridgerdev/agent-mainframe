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
      subtitle="The tmux session ended, but AMF still has the agent's saved session."
      onClose={onClose}
      dismissable={!busy}
      footer={sessions === null ? (
        <>
          <button className="btn btn-ghost" onClick={onClose}>Cancel</button>
          <button className="btn btn-secondary" disabled={busy} onClick={() => onRecover("clear", null)}>Start fresh</button>
          <button className="btn btn-primary" disabled={busy} onClick={() => onRecover("resume", null)}>
            {busy && <Spinner />}Resume saved session
          </button>
        </>
      ) : (
        <>
          <button className="btn btn-ghost" onClick={onBack}>Back</button>
          <button className="btn btn-primary" disabled={!selectedId || busy} onClick={() => onRecover("pick", selectedId)}>
            {busy && <Spinner />}Resume selected session
          </button>
        </>
      )}
    >
      {sessions === null ? (
        <>
          <p>Resume the saved session <span className="mono">{option.saved_id}</span>, start a new one, or choose another saved transcript.</p>
          <button className="btn btn-secondary recovery-choose" disabled={loading} onClick={onChoose}>
            {loading && <Spinner />}Choose another saved session
          </button>
        </>
      ) : sessions.length === 0 ? (
        <p className="muted">No saved sessions were found for this feature.</p>
      ) : (
        <div className="recovery-session-list">
          {sessions.map((session) => (
            <label key={session.id} className="recovery-session-row">
              <input type="radio" name="recovery-session" checked={selectedId === session.id}
                onChange={() => onSelect(session.id)} />
              <span><strong>{session.title}</strong><small className="mono">{session.id}</small></span>
            </label>
          ))}
        </div>
      )}
    </Modal>
  );
}
