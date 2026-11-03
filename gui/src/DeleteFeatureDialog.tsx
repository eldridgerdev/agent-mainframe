import { useState } from "react";
import type { TodoDeleteChoice, TodoHostChoice, TodoHostPrompt } from "./api";
import { Icon, Modal, Spinner } from "./ui";

const TODO_CHOICES: { value: TodoDeleteChoice; label: string; danger?: boolean }[] = [
  { value: "move_to_project", label: "Move them to the project list" },
  { value: "move_to_global", label: "Move them to the global list" },
  { value: "delete", label: "Delete them with the feature (cannot be undone)", danger: true },
];

const LISTS_HINT = "The project list is shared by every feature in this project; the global list is shared by all projects.";

/** Collect the worktree disposition and project-list host before deletion.
 * Both prompts come from backend preflight; closing applies neither choice. */
export default function DeleteFeatureDialog({
  projectName,
  featureName,
  isWorktree,
  unfinished,
  todoHost = null,
  busy,
  dormant = false,
  onConfirm,
  onClose,
}: {
  projectName: string;
  featureName: string;
  isWorktree: boolean;
  unfinished: number | null;
  todoHost?: TodoHostPrompt | null;
  busy: boolean;
  dormant?: boolean;
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
            ? " and deletes its separate checkout folder (worktree). Changes you have not committed there are lost. The git branch is kept."
            : ". It uses the project's own folder, which is left alone."}
        </p>
      </div>
      {dormant && <p className="muted">Each confirmation checks dormancy again. A feature that was opened, produced output, stopped or changed since the list loaded is refused. Cancel returns to Dormant features without deleting anything.</p>}
      {asking && (
        <fieldset className="new-session-options delete-feature-todos" disabled={busy}>
          <legend>
            {unfinished === 1
              ? "This feature's TODO list has 1 unfinished TODO."
              : `This feature's TODO list has ${unfinished} unfinished TODOs.`}
          </legend>
          <p className="field-hint">{LISTS_HINT}</p>
          {TODO_CHOICES.map((option) => (
            <label key={option.value} className={option.danger ? "new-session-option new-session-option-danger" : "new-session-option"}>
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
            The shared project TODO list ({todoHost.todo_count} {todoHost.todo_count === 1 ? "TODO" : "TODOs"}) is stored in
            this feature. Move it to another feature so you do not lose it.
          </legend>
          {!asking && <p className="field-hint">{LISTS_HINT}</p>}
          {todoHost.candidates.map((candidate) => (
            <label key={candidate.feature_id} className="new-session-option">
              <input type="radio" name="todo-host" checked={selectedHost === candidate.feature_id}
                onChange={() => setHostFeatureId(candidate.feature_id)} />
              <span>Move it to {candidate.name}</span>
            </label>
          ))}
          <label className="new-session-option new-session-option-danger">
            <input type="radio" name="todo-host" checked={selectedHost === null}
              onChange={() => setHostFeatureId(null)} />
            <span>Delete the project list and all of its TODOs (cannot be undone)</span>
          </label>
        </fieldset>
      )}
    </Modal>
  );
}
