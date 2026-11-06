import { useId, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { AgentSlug, CreateProjectRequest, HarnessInfo, allowedHarnesses, asGuiError, createProject } from "./api";
import { Field, Modal, Spinner } from "./ui";

import { BrowseButton, PathInputRow } from "./BrowseButton";

export const ALLOWED_HARNESSES_KEY = "allowed-harnesses";

/** The harnesses a creation form offers: the ones creation will accept for
 * this project, or the global choices before a project exists. Every
 * supported harness stands in until that answer arrives or if it cannot be
 * read; the backend still refuses a disallowed one. */
function useOfferedAgents(projectId: string | null, supported: HarnessInfo[]): HarnessInfo[] {
  const allowed = useQuery({
    queryKey: [ALLOWED_HARNESSES_KEY, projectId],
    queryFn: () => allowedHarnesses(projectId),
    retry: false,
  });
  return allowed.data ?? supported;
}

/** The last segment of a typed or picked path, used to suggest a project name. */
function folderName(path: string): string {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "";
}

export default function CreateProjectForm({
  agents,
  pending,
  error,
  onEdit,
  onCancel,
  onSubmit,
}: {
  agents: HarnessInfo[];
  pending: boolean;
  /** Why the last Create failed, shown inside the form rather than as a toast. */
  error: string | null;
  /** The user changed something, so the last failure no longer applies. */
  onEdit: () => void;
  onCancel: () => void;
  onSubmit: (request: CreateProjectRequest) => void;
}) {
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [nameEdited, setNameEdited] = useState(false);
  const [agent, setAgent] = useState<AgentSlug | "">("");
  // The last path checked on blur, and what the backend said about it.
  const [check, setCheck] = useState<{ path: string; ok: boolean; message: string } | null>(null);
  const offered = useOfferedAgents(null, agents);
  const selectedAgent = offered.some((candidate) => candidate.slug === agent) ? agent : "";
  const trimmed = path.trim();
  const pathCheck = check && check.path === trimmed ? check : null;

  // `candidate` is a path that has not reached state yet (a Browse… pick).
  async function checkPath(candidate: string = trimmed, currentName: string = name) {
    const target = candidate.trim();
    if (!target) { setCheck(null); return; }
    const projectName = currentName.trim() || folderName(target) || "project";
    try {
      // A dry run validates the folder and finds the repository root; it creates nothing.
      const response = await createProject({ path: target, project_name: projectName, preferred_agent: null, dry_run: true });
      setCheck({ path: target, ok: true, message: response.is_git
        ? `Git repository found at ${response.project_path}.`
        : "This folder is not a git repository. AMF can still use it, but features will share this folder instead of getting their own branch and checkout." });
    } catch (cause) {
      const message = asGuiError(cause).message;
      // A name clash is about the name field, not the folder.
      if (/already exists/.test(message)) setCheck({ path: target, ok: true, message: "" });
      else setCheck({ path: target, ok: false, message });
    }
  }

  function changePath(next: string): string {
    setPath(next);
    onEdit();
    const nextName = nameEdited ? name : folderName(next);
    if (!nameEdited) setName(nextName);
    return nextName;
  }
  const errorId = useId();

  return (
    <Modal
      label="New project"
      title="New project"
      subtitle="A project is a git repository on this computer. AMF runs agents in it, one feature at a time."
      onClose={onCancel}
      onSubmit={() => onSubmit({
        path: trimmed,
        project_name: name.trim(),
        preferred_agent: selectedAgent || null,
        dry_run: false,
      })}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onCancel}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={pending}>
            {pending && <Spinner />}
            {pending ? "Creating…" : "Create project"}
          </button>
        </>
      }
    >
      <div className="form-stack">
        <Field label="Repository path" hint={pathCheck?.message
          ? <span className={pathCheck.ok ? undefined : "field-error"} role={pathCheck.ok ? "status" : "alert"}>{pathCheck.message}</span>
          : "The folder of a git repository, for example the one you cloned."}>
          <PathInputRow browse={
            <BrowseButton
              kind="directory"
              label="Browse for repository folder"
              dialogTitle="Choose a repository folder"
              defaultPath={trimmed}
              disabled={pending}
              // A pick is checked the same way leaving the field is.
              onPick={(picked) => { const nextName = changePath(picked); void checkPath(picked, nextName); }}
            />
          }>
            <input
              className="mono"
              value={path}
              onChange={(event) => { changePath(event.target.value); }}
              onBlur={() => void checkPath()}
              placeholder="/home/you/code/my-repo"
              required
              autoFocus
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? errorId : undefined}
            />
          </PathInputRow>
        </Field>
        {error && (
          <p id={errorId} className="field-error" role="alert">
            <strong>Could not add the project.</strong> {error}
          </p>
        )}
        <Field label="Project name">
          <input
            value={name}
            onChange={(event) => {
              setName(event.target.value);
              setNameEdited(true);
              onEdit();
            }}
            required
          />
        </Field>
        <Field label="Preferred agent" hint="The agent new features use by default. You can change it per feature.">
          <select value={selectedAgent} onChange={(event) => setAgent(event.target.value as AgentSlug)}>
            <option value="">Default</option>
            {offered.map((a) => (
              <option key={a.slug} value={a.slug}>{a.display_name}</option>
            ))}
          </select>
        </Field>
      </div>
    </Modal>
  );
}
