import { useState } from "react";
import type { TodoDeleteChoice, TodoHostChoice, TodoHostPrompt } from "./api";
import { Icon, Modal, Spinner } from "./ui";

const TODO_CHOICES: { value: TodoDeleteChoice; label: string }[] = [
  { value: "move_to_project", label: "Move them to the project list" },
  { value: "move_to_global", label: "Move them to the global list" },
  { value: "delete", label: "Delete them with the worktree" },
];

/** Collect the worktree disposition and project-list host before deletion.
 * Both prompts come from backend preflight; closing applies neither choice. */
export default function DeleteFeatureDialog({
  projectName,
  featureName,
  isWorktree,
  unfinished,
  todoHost = null,
  busy,
  onConfirm,
  onClose,
}: {
  projectName: string;
  featureName: string;
  isWorktree: boolean;
  unfinished: number | null;
  todoHost?: TodoHostPrompt | null;
  busy: boolean;
  onConfirm: (todos: TodoDeleteChoice | null, todoHost: TodoHostChoice | null) => void;
  onClose: () => void;
}) {
  const [choice, setChoice] = useState<TodoDeleteChoice>("move_to_project");
  const [hostFeatureId, setHostFeatureId] = useState<string | null | undefined>(undefined);
  const selectedHost = hostFeatureId === undefined ? todoHost?.candidates[0]?.feature_id : hostFeatureId;
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
        if (!busy) onConfirm(asking ? choice : null, todoHost ? {
          list_id: todoHost.list_id,
          feature_id: selectedHost ?? null,
        } : null);
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
        <fieldset className="new-session-options delete-feature-todos" disabled={busy}>
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
      {todoHost && (
        <fieldset className="new-session-options delete-feature-todos" disabled={busy}>
          <legend>
            This feature hosts the project list ({todoHost.todo_count} {todoHost.todo_count === 1 ? "TODO" : "TODOs"}).
            Choose where to keep it, or delete the list.
          </legend>
          {todoHost.candidates.map((candidate) => (
            <label key={candidate.feature_id} className="new-session-option">
              <input type="radio" name="todo-host" checked={selectedHost === candidate.feature_id}
                onChange={() => setHostFeatureId(candidate.feature_id)} />
              <span>Keep on {candidate.name}</span>
            </label>
          ))}
          <label className="new-session-option">
            <input type="radio" name="todo-host" checked={selectedHost === null}
              onChange={() => setHostFeatureId(null)} />
            <span>Delete the project list and all of its TODOs</span>
          </label>
        </fieldset>
      )}
    </Modal>
  );
}
