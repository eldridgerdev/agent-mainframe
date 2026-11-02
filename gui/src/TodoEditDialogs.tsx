import { useState } from "react";
import { Todo, TodoEditOutcome, TodoFields, TodoPriority } from "./api";
import { Field, Icon, Modal, Segmented } from "./ui";

const PRIORITIES: { value: TodoPriority; label: string }[] = [
  { value: "high", label: "High" },
  { value: "med", label: "Medium" },
  { value: "low", label: "Low" },
];

const fieldsOf = (todo: Todo): TodoFields => ({
  title: todo.title,
  body: todo.body,
  priority: todo.priority,
});

const samePad = (a: string | null, b: string | null) => (a ?? "") === (b ?? "");

/** Edit a TODO's title, notes and priority. The save is bound to the values
 * the editor opened with; a refused save shows what is stored now and leaves
 * the choice (take theirs, or deliberately re-apply mine) to the user. */
export function TodoEditDialog({
  todo,
  onSave,
  onCancel,
}: {
  todo: Todo;
  onSave: (base: TodoFields, fields: TodoFields) => Promise<TodoEditOutcome>;
  onCancel: () => void;
}) {
  const [base, setBase] = useState<TodoFields>(fieldsOf(todo));
  const [title, setTitle] = useState(todo.title);
  const [body, setBody] = useState(todo.body ?? "");
  const [priority, setPriority] = useState<TodoPriority>(todo.priority);
  const [conflict, setConflict] = useState<Todo | null>(null);
  const [busy, setBusy] = useState(false);
  const fields: TodoFields = { title, body: body.trim() ? body : null, priority };
  const dirty = title !== base.title || (fields.body ?? "") !== (base.body ?? "") || priority !== base.priority;

  async function save(against: TodoFields) {
    setBusy(true);
    try {
      const result = await onSave(against, fields);
      if (result.outcome === "conflict") {
        setConflict(result.current);
      } else {
        onCancel();
      }
    } catch {
      // The panel already reported the failure; keep the dialog open.
    } finally {
      setBusy(false);
    }
  }

  function takeTheirs(current: Todo) {
    const next = fieldsOf(current);
    setBase(next);
    setTitle(next.title);
    setBody(next.body ?? "");
    setPriority(next.priority);
    setConflict(null);
  }

  return (
    <Modal
      label="Edit TODO"
      title="Edit TODO"
      onClose={onCancel}
      footer={
        <>
          <button className="btn btn-ghost" onClick={onCancel}>Cancel</button>
          {conflict ? (
            <button
              className="btn btn-primary"
              disabled={busy || !title.trim()}
              onClick={() => void save(fieldsOf(conflict))}
            >
              Overwrite with my edits
            </button>
          ) : (
            <button
              className="btn btn-primary"
              disabled={busy || !title.trim() || !dirty}
              onClick={() => void save(base)}
            >
              Save
            </button>
          )}
        </>
      }
    >
      <div className="form-stack" data-unsaved-changes={dirty}>
        {conflict && (
          <div className="callout callout-warning" role="alert">
            <Icon name="alert" size={14} />
            <div>
              <strong>This TODO changed since you opened it.</strong>
              <p className="small">Nothing was saved. It currently reads:</p>
              <p className="small"><b>{conflict.title}</b> ({conflict.priority})</p>
              {conflict.body && <p className="small todo-notes">{conflict.body}</p>}
              <button className="btn btn-sm btn-secondary" onClick={() => takeTheirs(conflict)}>
                Discard my edits and use this
              </button>
            </div>
          </div>
        )}
        <Field label="Title">
          <input value={title} onChange={(event) => setTitle(event.target.value)} aria-label="TODO title" />
        </Field>
        <Field label="Notes" hint="Sent to the agent with the title when this TODO is started.">
          <textarea
            className="plan-textarea todo-notes-input"
            rows={6}
            value={body}
            aria-label="TODO notes"
            onChange={(event) => setBody(event.target.value)}
          />
        </Field>
        <Field label="Priority">
          <Segmented label="Priority" value={priority} onChange={setPriority} options={PRIORITIES} />
        </Field>
      </div>
    </Modal>
  );
}

/** Edit a list's scratchpad ("left off here" note), bound to the text the
 * editor opened with. */
export function ScratchpadDialog({
  title,
  base,
  onSave,
  onCancel,
}: {
  title: string;
  base: string | null;
  onSave: (
    base: string | null,
    text: string | null,
  ) => Promise<{ outcome: "saved" } | { outcome: "conflict"; current: string | null } | { outcome: "gone" }>;
  onCancel: () => void;
}) {
  const [against, setAgainst] = useState(base);
  const [text, setText] = useState(base ?? "");
  const [stored, setStored] = useState<{ current: string | null } | null>(null);
  const [busy, setBusy] = useState(false);
  const dirty = !samePad(text, against);

  async function save(baseValue: string | null) {
    setBusy(true);
    try {
      const result = await onSave(baseValue, text.trim() ? text : null);
      if (result.outcome === "conflict") setStored({ current: result.current });
      else onCancel();
    } catch {
      // The panel already reported the failure; keep the dialog open.
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      label="Edit scratchpad"
      title="Scratchpad"
      subtitle={title}
      onClose={onCancel}
      footer={
        <>
          <button className="btn btn-ghost" onClick={onCancel}>Cancel</button>
          <button
            className="btn btn-primary"
            disabled={busy || !dirty}
            onClick={() => void save(stored ? stored.current : against)}
          >
            {stored ? "Overwrite with my edits" : "Save"}
          </button>
        </>
      }
    >
      <div className="form-stack" data-unsaved-changes={dirty}>
        {stored && (
          <div className="callout callout-warning" role="alert">
            <Icon name="alert" size={14} />
            <div>
              <strong>The scratchpad changed since you opened it.</strong>
              <p className="small">Nothing was saved. It currently reads:</p>
              <p className="small todo-notes">{stored.current ?? "(empty)"}</p>
              <button
                className="btn btn-sm btn-secondary"
                onClick={() => {
                  setAgainst(stored.current);
                  setText(stored.current ?? "");
                  setStored(null);
                }}
              >
                Discard my edits and use this
              </button>
            </div>
          </div>
        )}
        <Field label="Scratchpad" hint="A free-form note for where you left off. Shared with the TUI.">
          <textarea
            className="plan-textarea todo-notes-input"
            rows={8}
            value={text}
            aria-label="Scratchpad text"
            onChange={(event) => setText(event.target.value)}
          />
        </Field>
      </div>
    </Modal>
  );
}
