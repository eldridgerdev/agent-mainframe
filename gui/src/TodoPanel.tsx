import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  GuiError,
  FeatureTarget,
  Todo,
  TodoScopeRequest,
  TodoStatus,
  asGuiError,
  todoAdd,
  todoDelete,
  todoCopy,
  todoList,
  todoMove,
  todoReorder,
  todoSetStatus,
} from "./api";
import { Field, Icon, Menu, Modal, Segmented, Spinner } from "./ui";

export interface TodoDestination {
  label: string;
  scope: TodoScopeRequest;
}

export interface TodoAgentTarget {
  label: string;
  target: FeatureTarget;
}

const STATUS_CYCLE: Record<TodoStatus, TodoStatus> = {
  not_started: "in_progress",
  in_progress: "completed",
  completed: "not_started",
};

const STATUS_TEXT: Record<TodoStatus, string> = {
  not_started: "Not started",
  in_progress: "In progress",
  completed: "Done",
};

const NEW_FEATURE = "__new__";

const targetKey = (target: FeatureTarget) => `${target.project_id}:${target.feature_id}`;

// One panel for global, project, and feature TODO scopes. All mutations use
// the same `gui_todos`/`db::todos` operations as the TUI.
export default function TodoPanel({
  scope,
  hostFeatureId,
  title,
  description,
  onError,
  destinations = [],
  launchTargets = [],
  onLaunch,
  onPlan,
  onPlanNew,
  onLaunchNew,
}: {
  scope: TodoScopeRequest;
  hostFeatureId?: string;
  title: string;
  description?: string;
  onError: (err: GuiError) => void;
  destinations?: TodoDestination[];
  launchTargets?: TodoAgentTarget[];
  onLaunch?: (todoId: string, target: FeatureTarget) => Promise<void>;
  onPlan?: (todoId: string, target: FeatureTarget) => Promise<void>;
  onPlanNew?: (todoId: string, title: string) => void;
  onLaunchNew?: (todoId: string, title: string) => void;
}) {
  const queryClient = useQueryClient();
  const queryKey = ["todos", JSON.stringify(scope)];
  const [draft, setDraft] = useState("");
  const [launchingTodoId, setLaunchingTodoId] = useState<string | null>(null);
  const [startDialog, setStartDialog] = useState<Todo | null>(null);
  const [transferDialog, setTransferDialog] = useState<Todo | null>(null);

  const view = useQuery({
    queryKey,
    queryFn: () => todoList(scope),
    refetchInterval: 2_000,
  });

  function invalidate() {
    void queryClient.invalidateQueries({ queryKey: ["todos"] });
  }

  const addMutation = useMutation({
    mutationFn: (draftTitle: string) =>
      todoAdd(scope, draftTitle, { hostFeatureId }),
    onSuccess: () => {
      setDraft("");
      invalidate();
    },
    onError: (err) => onError(asGuiError(err)),
  });

  const statusMutation = useMutation({
    mutationFn: ({ todoId, status }: { todoId: string; status: TodoStatus }) =>
      todoSetStatus(todoId, status),
    onSuccess: invalidate,
    onError: (err) => onError(asGuiError(err)),
  });

  const deleteMutation = useMutation({
    mutationFn: todoDelete,
    onSuccess: invalidate,
    onError: (err) => onError(asGuiError(err)),
  });

  const reorderMutation = useMutation({
    mutationFn: todoReorder,
    onSuccess: invalidate,
    onError: (err) => onError(asGuiError(err)),
  });

  const transferMutation = useMutation({
    mutationFn: ({ todoId, target, copy }: {
      todoId: string;
      target: TodoScopeRequest;
      copy: boolean;
    }) => copy ? todoCopy(todoId, target) : todoMove(todoId, target),
    onSuccess: () => {
      setTransferDialog(null);
      invalidate();
    },
    onError: (err) => onError(asGuiError(err)),
  });

  const todos: Todo[] = view.data?.todos ?? [];
  const otherScopes = destinations.filter(
    (destination) => JSON.stringify(destination.scope) !== JSON.stringify(scope),
  );
  const openCount = todos.filter((todo) => todo.work.status !== "completed").length;
  const canStart = Boolean(
    (launchTargets.length > 0 && (onLaunch || onPlan)) || onLaunchNew || onPlanNew,
  );

  function reorder(index: number, delta: number) {
    const ids = todos.map((todo) => todo.id);
    const next = index + delta;
    if (next < 0 || next >= ids.length) return;
    [ids[index], ids[next]] = [ids[next], ids[index]];
    reorderMutation.mutate(ids);
  }

  function runLaunch(todoId: string, run: () => Promise<void>) {
    setLaunchingTodoId(todoId);
    void run().finally(() => setLaunchingTodoId(null));
  }

  return (
    <section className="todo-panel" aria-label={title}>
      <header className="panel-header">
        <div>
          <h3>{title}</h3>
          {description && <p className="panel-description">{description}</p>}
        </div>
        {todos.length > 0 && (
          <span className="count-pill">{openCount} open</span>
        )}
      </header>

      <form
        className="todo-add"
        onSubmit={(event) => {
          event.preventDefault();
          if (draft.trim()) addMutation.mutate(draft.trim());
        }}
      >
        <Icon name="plus" />
        <input
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          placeholder="Add a TODO and press Enter"
          aria-label={`Add to ${title}`}
        />
        {draft.trim() && (
          <button type="submit" className="btn btn-sm btn-primary" disabled={addMutation.isPending}>
            Add
          </button>
        )}
      </form>

      {view.isLoading && <p className="muted small pad">Loading…</p>}
      {view.error && <p role="alert" className="error-text pad">{String(view.error)}</p>}
      {!view.isLoading && todos.length === 0 && (
        <p className="todo-empty">Nothing here yet.</p>
      )}

      {todos.length > 0 && (
        <ul className="todo-list">
          {todos.map((todo, index) => {
            const status = todo.work.status;
            const launching = launchingTodoId === todo.id;
            const menuItems = [
              { label: "Move up", icon: "arrowUp" as const, disabled: index === 0 || reorderMutation.isPending, onSelect: () => reorder(index, -1) },
              { label: "Move down", icon: "arrowDown" as const, disabled: index === todos.length - 1 || reorderMutation.isPending, onSelect: () => reorder(index, 1) },
              ...(otherScopes.length > 0
                ? [{ label: "Move or copy…", icon: "swap" as const, onSelect: () => setTransferDialog(todo) }]
                : []),
              { label: "Delete", icon: "trash" as const, danger: true, onSelect: () => deleteMutation.mutate(todo.id) },
            ];
            return (
              <li key={todo.id} className={`todo todo-${status}`}>
                <button
                  className="todo-check"
                  onClick={() =>
                    statusMutation.mutate({ todoId: todo.id, status: STATUS_CYCLE[status] })
                  }
                  title={`${STATUS_TEXT[status]} — click to change`}
                  aria-label={`${todo.title}: ${STATUS_TEXT[status]}`}
                >
                  {status === "completed" && <Icon name="check" size={12} />}
                </button>
                <div className="todo-main">
                  <span className="todo-title">{todo.title}</span>
                  {(status === "in_progress" || todo.priority === "high") && (
                    <span className="todo-meta">
                      {status === "in_progress" && <span className="tag tag-accent">In progress</span>}
                      {todo.priority === "high" && <span className="tag tag-red">High</span>}
                    </span>
                  )}
                </div>
                <div className="todo-actions">
                  {canStart && status === "not_started" && (
                    <button
                      className="btn btn-sm btn-secondary"
                      disabled={launching}
                      onClick={() => setStartDialog(todo)}
                    >
                      {launching ? <Spinner /> : <Icon name="play" size={12} />}
                      {launching ? "Starting…" : "Start"}
                    </button>
                  )}
                  <Menu label={`Actions for ${todo.title}`} items={menuItems} className="btn btn-icon btn-ghost btn-sm" />
                </div>
              </li>
            );
          })}
        </ul>
      )}

      {startDialog && (
        <StartTodoDialog
          todo={startDialog}
          launchTargets={onLaunch || onPlan ? launchTargets : []}
          canLaunch={Boolean(onLaunch)}
          canPlan={Boolean(onPlan)}
          canLaunchNew={Boolean(onLaunchNew)}
          canPlanNew={Boolean(onPlanNew)}
          onCancel={() => setStartDialog(null)}
          onConfirm={(where, how) => {
            const todo = startDialog;
            setStartDialog(null);
            if (where === NEW_FEATURE) {
              if (how === "plan") onPlanNew?.(todo.id, todo.title);
              else onLaunchNew?.(todo.id, todo.title);
              return;
            }
            const target = launchTargets.find((candidate) => targetKey(candidate.target) === where);
            if (!target) return;
            if (how === "plan" && onPlan) runLaunch(todo.id, () => onPlan(todo.id, target.target));
            else if (onLaunch) runLaunch(todo.id, () => onLaunch(todo.id, target.target));
          }}
        />
      )}

      {transferDialog && (
        <TransferTodoDialog
          todo={transferDialog}
          destinations={otherScopes}
          busy={transferMutation.isPending}
          onCancel={() => setTransferDialog(null)}
          onConfirm={(target, copy) =>
            transferMutation.mutate({ todoId: transferDialog.id, target, copy })
          }
        />
      )}
    </section>
  );
}

function StartTodoDialog({
  todo,
  launchTargets,
  canLaunch,
  canPlan,
  canLaunchNew,
  canPlanNew,
  onCancel,
  onConfirm,
}: {
  todo: Todo;
  launchTargets: TodoAgentTarget[];
  canLaunch: boolean;
  canPlan: boolean;
  canLaunchNew: boolean;
  canPlanNew: boolean;
  onCancel: () => void;
  onConfirm: (where: string, how: "launch" | "plan") => void;
}) {
  const [where, setWhere] = useState(
    launchTargets[0] ? targetKey(launchTargets[0].target) : NEW_FEATURE,
  );
  const isNew = where === NEW_FEATURE;
  const allowLaunch = isNew ? canLaunchNew : canLaunch;
  const allowPlan = isNew ? canPlanNew : canPlan;
  const [how, setHow] = useState<"launch" | "plan">(allowLaunch ? "launch" : "plan");
  const effectiveHow = how === "plan" && !allowPlan ? "launch" : how === "launch" && !allowLaunch ? "plan" : how;

  return (
    <Modal
      label="Start TODO"
      title="Start work on a TODO"
      subtitle={todo.title}
      onClose={onCancel}
      footer={
        <>
          <button className="btn btn-ghost" onClick={onCancel}>Cancel</button>
          <button className="btn btn-primary" onClick={() => onConfirm(where, effectiveHow)}>
            {effectiveHow === "plan" ? <Icon name="sparkles" /> : <Icon name="play" size={12} />}
            {effectiveHow === "plan" ? "Start planning" : "Start agent"}
          </button>
        </>
      }
    >
      <div className="form-stack">
        <Field
          label="Where"
          hint={isNew ? "Creates a new git worktree feature for this TODO." : undefined}
        >
          <select value={where} onChange={(event) => setWhere(event.target.value)}>
            {launchTargets.map((candidate) => (
              <option key={targetKey(candidate.target)} value={targetKey(candidate.target)}>
                {candidate.label}
              </option>
            ))}
            {(canLaunchNew || canPlanNew) && (
              <option value={NEW_FEATURE}>New feature…</option>
            )}
          </select>
        </Field>
        {allowLaunch && allowPlan && (
          <Field
            label="How"
            hint={effectiveHow === "plan"
              ? "Run a plan interview first; the agent starts once you accept the plan."
              : "Start the agent now. Its prompt opens as an editable draft before sending."}
          >
            <Segmented
              label="How to start"
              value={effectiveHow}
              onChange={setHow}
              options={[
                { value: "launch", label: "Start agent" },
                { value: "plan", label: "Plan first" },
              ]}
            />
          </Field>
        )}
      </div>
    </Modal>
  );
}

function TransferTodoDialog({
  todo,
  destinations,
  busy,
  onCancel,
  onConfirm,
}: {
  todo: Todo;
  destinations: TodoDestination[];
  busy: boolean;
  onCancel: () => void;
  onConfirm: (target: TodoScopeRequest, copy: boolean) => void;
}) {
  const [key, setKey] = useState(JSON.stringify(destinations[0]?.scope));
  const destination = destinations.find((candidate) => JSON.stringify(candidate.scope) === key);
  return (
    <Modal
      label="Move or copy TODO"
      title="Move or copy"
      subtitle={todo.title}
      size="sm"
      onClose={onCancel}
      footer={
        <>
          <button className="btn btn-ghost" onClick={onCancel}>Cancel</button>
          <button
            className="btn btn-secondary"
            disabled={busy || !destination}
            onClick={() => destination && onConfirm(destination.scope, true)}
          >Copy</button>
          <button
            className="btn btn-primary"
            disabled={busy || !destination}
            onClick={() => destination && onConfirm(destination.scope, false)}
          >Move</button>
        </>
      }
    >
      <Field
        label="Destination"
        hint="Moving keeps its status and agent link. A copy starts fresh."
      >
        <select aria-label={`Destination for ${todo.title}`} value={key} onChange={(event) => setKey(event.target.value)}>
          {destinations.map((candidate) => (
            <option key={JSON.stringify(candidate.scope)} value={JSON.stringify(candidate.scope)}>
              {candidate.label}
            </option>
          ))}
        </select>
      </Field>
    </Modal>
  );
}
