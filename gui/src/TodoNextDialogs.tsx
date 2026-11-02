import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  asGuiError,
  FeatureTarget,
  GuiError,
  NextTodoCandidate,
  NextTodoRequest,
  todoNextPreview,
  todoQuickCapture,
  todoQuickCaptureTarget,
} from "./api";
import { Field, Icon, Modal, Spinner } from "./ui";

const PRIORITY_TEXT = { high: "High", med: "Medium", low: "Low" } as const;

/** "Implement next": shows which TODO the shared rule picks (priority, then
 * worktree before project before global, held-in-reserve for already-started
 * items) and what acting on it means. Choosing it hands the TODO to the
 * ordinary Start dialog, which asks for the target and goes through the same
 * resource approval and reservation as any other start. */
export function NextTodoDialog({
  base,
  canStart,
  canJump,
  onStart,
  onJump,
  onClose,
}: {
  base: Pick<NextTodoRequest, "project_id" | "feature_id">;
  canStart: boolean;
  canJump: boolean;
  onStart: (candidate: NextTodoCandidate) => void;
  onJump: (candidate: NextTodoCandidate) => void;
  onClose: () => void;
}) {
  const [includeProject, setIncludeProject] = useState(true);
  const [includeGlobal, setIncludeGlobal] = useState(true);
  const [skipped, setSkipped] = useState<string[]>([]);
  const request: NextTodoRequest = {
    ...base,
    include_project: includeProject,
    include_global: includeGlobal,
    skipped_ids: skipped,
  };
  const preview = useQuery({
    queryKey: ["todo-next", JSON.stringify(request)],
    queryFn: () => todoNextPreview(request),
    gcTime: 0,
    staleTime: 0,
  });
  const data = preview.data;
  const candidate = data && data.outcome !== "empty" ? data.candidate : null;
  const canProject = Boolean(base.project_id);

  return (
    <Modal
      label="Implement next TODO"
      title="Implement next"
      subtitle="The highest-priority unstarted TODO across the lists below."
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-ghost" onClick={onClose}>Close</button>
          {candidate && (
            <button className="btn btn-secondary" onClick={() => setSkipped([...skipped, candidate.todo.id])}>
              Skip to next
            </button>
          )}
          {candidate && data?.outcome === "started" && canJump && (
            <button className="btn btn-secondary" onClick={() => onJump(candidate)}>
              Go to work
            </button>
          )}
          {candidate && canStart && (
            <button className="btn btn-primary" onClick={() => onStart(candidate)}>
              <Icon name="play" size={12} />
              {data?.outcome === "started" ? "Start another agent…" : "Start…"}
            </button>
          )}
        </>
      }
    >
      <div className="form-stack">
        <div className="todo-next-scopes" role="group" aria-label="Lists to include">
          <label className="check-row"><input type="checkbox" checked disabled /> Worktree list (when this is a worktree feature)</label>
          <label className="check-row">
            <input type="checkbox" checked={includeProject && canProject} disabled={!canProject}
              onChange={(event) => { setIncludeProject(event.target.checked); setSkipped([]); }} />
            Project list
          </label>
          <label className="check-row">
            <input type="checkbox" checked={includeGlobal}
              onChange={(event) => { setIncludeGlobal(event.target.checked); setSkipped([]); }} />
            Global list
          </label>
        </div>
        {preview.isLoading && <p className="muted"><Spinner /> Looking…</p>}
        {preview.error && <p role="alert" className="error-text">{asGuiError(preview.error).message}</p>}
        {data?.outcome === "empty" && <p className="todo-empty">{data.message}</p>}
        {candidate && (
          <div className="todo-next-card" aria-label="Next TODO">
            <strong>{candidate.todo.title}</strong>
            <p className="small muted">
              {candidate.scope_label} · {PRIORITY_TEXT[candidate.todo.priority]} priority
            </p>
            {candidate.todo.body && <p className="small todo-notes">{candidate.todo.body}</p>}
            {data?.outcome === "started" && (
              <p className="small" role="note">
                {data.reason} Nothing unstarted is left, so this is offered rather than started.
              </p>
            )}
            {candidate.scope_kind !== "worktree" && (
              <p className="small muted">You choose which feature runs it; nothing starts until you confirm.</p>
            )}
          </div>
        )}
      </div>
    </Modal>
  );
}

/** One-line capture into the list this feature writes to by default. */
export function QuickTodoDialog({
  target,
  onSaved,
  onError,
  onClose,
}: {
  target: FeatureTarget;
  onSaved: (listLabel: string) => void;
  onError: (error: GuiError) => void;
  onClose: () => void;
}) {
  const [title, setTitle] = useState("");
  const [label, setLabel] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const saving = useRef(false);

  useEffect(() => {
    let live = true;
    todoQuickCaptureTarget(target)
      .then((name) => live && setLabel(name))
      .catch((err) => onError(asGuiError(err)));
    return () => { live = false; };
  }, [target.project_id, target.feature_id]);

  async function save() {
    if (!title.trim() || saving.current) return;
    saving.current = true;
    setBusy(true);
    try {
      const saved = await todoQuickCapture(target, title);
      onSaved(saved.scope_label);
      onClose();
    } catch (err) {
      onError(asGuiError(err));
    } finally {
      saving.current = false;
      setBusy(false);
    }
  }

  return (
    <Modal
      label="Quick TODO"
      title="Quick TODO"
      subtitle={label ? `Adds to ${label}` : "Finding the list…"}
      size="sm"
      onClose={onClose}
      onSubmit={() => void save()}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose}>Cancel</button>
          {/* A submit button: the form's onSubmit saves exactly once. */}
          <button type="submit" className="btn btn-primary" disabled={busy || !title.trim() || !label}>
            Add TODO
          </button>
        </>
      }
    >
      <div data-unsaved-changes={title.length > 0}>
        <Field label="TODO" hint="One line. Add notes later with Edit.">
          <input autoFocus value={title} aria-label="Quick TODO title" onChange={(event) => setTitle(event.target.value)} />
        </Field>
      </div>
    </Modal>
  );
}
