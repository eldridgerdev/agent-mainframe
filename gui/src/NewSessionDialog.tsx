import { useState } from "react";
import type { NewSessionKind, NewSessionOption } from "./api";
import { Modal, Spinner } from "./ui";

export default function NewSessionDialog({
  options,
  preferredKind,
  busy,
  onCreate,
  onClose,
}: {
  options: NewSessionOption[];
  preferredKind: NewSessionKind;
  busy: boolean;
  onCreate: (kind: NewSessionKind, label: string | null) => void;
  onClose: () => void;
}) {
  const [kind, setKind] = useState<NewSessionKind>(() =>
    options.some((option) => option.kind === preferredKind)
      ? preferredKind
      : options[0]?.kind ?? "terminal",
  );
  const [label, setLabel] = useState("");

  return (
    <Modal
      label="New session"
      title="New session"
      subtitle="Open another agent or terminal in this feature."
      onClose={onClose}
      dismissable={!busy}
      onSubmit={() => {
        if (!busy && options.length > 0) onCreate(kind, label.trim() || null);
      }}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose} disabled={busy}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={busy || options.length === 0}>
            {busy && <Spinner />}Create session
          </button>
        </>
      }
    >
      <fieldset className="new-session-options">
        <legend>Session type</legend>
        {options.map((option) => (
          <label key={option.kind} className="new-session-option">
            <input type="radio" name="new-session-kind" value={option.kind}
              checked={kind === option.kind} onChange={() => setKind(option.kind)} />
            <span>{option.label}</span>
          </label>
        ))}
      </fieldset>
      <label className="field">
        <span>Session name <span className="muted">(optional)</span></span>
        <input type="text" value={label} onChange={(event) => setLabel(event.target.value)}
          placeholder="Use the next default name" maxLength={80} />
      </label>
    </Modal>
  );
}
