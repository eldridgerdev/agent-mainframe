import { useState } from "react";
import type { TodoDeleteChoice } from "./api";
import { Icon, Modal, Spinner } from "./ui";

const TODO_CHOICES: { value: TodoDeleteChoice; label: string }[] = [
  { value: "move_to_project", label: "Move them to the project list" },
  { value: "move_to_global", label: "Move them to the global list" },
  { value: "delete", label: "Delete them with the worktree" },
];

/** The TUI's delete confirm plus its TODO disposition prompt. `unfinished`
 * is null until the backend has said the worktree list needs a decision. */
export default function DeleteFeatureDialog({
  projectName,
  featureName,
  isWorktree,
  unfinished,
  busy,
  onConfirm,
  onClose,
}: {
  projectName: string;
  featureName: string;
  isWorktree: boolean;
  unfinished: number | null;
  busy: boolean;
  onConfirm: (todos: TodoDeleteChoice | null) => void;
  onClose: () => void;
}) {
  const [choice, setChoice] = useState<TodoDeleteChoice>("move_to_project");
  const asking = unfinished !== null;

  return (
    <Modal
      label="Delete feature"
      title={`Delete ${featureName}?`}
      subtitle={`From ${projectName}`}
      size="sm"
      onClose={onClose}
      dismissable={!busy}
      onSubmit={() => {
        if (!busy) onConfirm(asking ? choice : null);
      }}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose} disabled={busy}>Cancel</button>
          <button type="submit" className="btn btn-danger" disabled={busy}>
            {busy && <Spinner />}Delete feature
          </button>
        </>
      }
    >
      <div className="callout callout-warning">
        <Icon name="alert" />
        <p>
          This stops all of its sessions
          {isWorktree
            ? " and removes the worktree. Uncommitted changes in it are discarded; the branch is kept."
            : ". The repository checkout itself is left alone."}
        </p>
      </div>
      {asking && (
        <fieldset className="new-session-options delete-feature-todos">
          <legend>
            {unfinished === 1
              ? "Its worktree has 1 unfinished TODO."
              : `Its worktree has ${unfinished} unfinished TODOs.`}
          </legend>
          {TODO_CHOICES.map((option) => (
            <label key={option.value} className="new-session-option">
              <input type="radio" name="delete-feature-todos" value={option.value}
                checked={choice === option.value} onChange={() => setChoice(option.value)} />
              <span>{option.label}</span>
            </label>
          ))}
        </fieldset>
      )}
    </Modal>
  );
}
