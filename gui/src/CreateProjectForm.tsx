import { useEffect, useId, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  AgentSlug, CLONE_PROJECT_EVENT, CloneFix, CloneProjectSnapshot, CreateProjectRequest, HarnessInfo,
  allowedHarnesses, asGuiError, cloneProjectCancel, cloneProjectDerive, cloneProjectPreflight,
  cloneProjectReset, cloneProjectSnapshot, cloneProjectStart, createProject,
} from "./api";
import { CloneProgressView, FailureNotice, PreflightChecklist } from "./CloneProjectPanels";
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

/** Derivation waits for typing to settle so each keystroke is not a command. */
const DERIVE_DEBOUNCE_MS = 250;
/** Safety net under the `clone_project_changed` event. */
const CLONE_POLL_MS = 1500;

const RUNNING = new Set(["preflight_running", "cloning", "registering"]);

export default function CreateProjectForm({
  agents,
  pending,
  error,
  onEdit,
  onCancel,
  onSubmit,
  onCloned,
}: {
  agents: HarnessInfo[];
  pending: boolean;
  /** Why the last Create failed, shown inside the form rather than as a toast. */
  error: string | null;
  /** The user changed something, so the last failure no longer applies. */
  onEdit: () => void;
  onCancel: () => void;
  onSubmit: (request: CreateProjectRequest) => void;
  /** A clone finished and the project is registered. */
  onCloned?: (project: { name: string; dest: string }) => void;
}) {
  const [url, setUrl] = useState("");
  const [dest, setDest] = useState("");
  const [destEdited, setDestEdited] = useState(false);
  const [urlError, setUrlError] = useState<string | null>(null);
  const [snap, setSnap] = useState<CloneProjectSnapshot | null>(null);
  const [cloneError, setCloneError] = useState<string | null>(null);
  const [failure, setFailure] = useState<{ message: string; advice: string | null; fixes: CloneFix[] } | null>(null);
  const [busy, setBusy] = useState(false);
  const clonedReported = useRef(false);
  const cloneMode = url.trim() !== "";
  const phase = snap?.phase.kind ?? "idle";
  const inFlow = phase === "preflight_running" || phase === "preflight_done" || phase === "cloning"
    || phase === "registering" || phase === "succeeded";
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

  // The backend alone derives the name and folder; a field the user has
  // edited is never overwritten.
  useEffect(() => {
    const typed = url.trim();
    if (!typed) { setUrlError(null); return; }
    let stale = false;
    const timer = setTimeout(() => {
      cloneProjectDerive(typed).then(
        (derived) => {
          if (stale) return;
          setUrlError(null);
          if (!nameEdited) setName(derived.name);
          if (!destEdited) setDest(derived.dest);
        },
        (cause) => { if (!stale) setUrlError(asGuiError(cause).message); },
      );
    }, DERIVE_DEBOUNCE_MS);
    return () => { stale = true; clearTimeout(timer); };
    // Edits to name/dest must not re-derive; only the URL does.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [url]);

  // Progress arrives as an event; a slow poll covers one that is missed.
  useEffect(() => {
    if (!snap || phase === "idle") return;
    let stopped = false;
    const unlisten = listen<CloneProjectSnapshot>(CLONE_PROJECT_EVENT, (event) => {
      if (!stopped) setSnap(event.payload);
    });
    const timer = RUNNING.has(phase)
      ? setInterval(() => { void cloneProjectSnapshot().then((next) => { if (!stopped && next) setSnap(next); }, () => undefined); }, CLONE_POLL_MS)
      : undefined;
    return () => {
      stopped = true;
      if (timer) clearInterval(timer);
      void Promise.resolve(unlisten).then((off) => off?.());
    };
    // Re-subscribe only when the flow starts or leaves the running phases.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase === "idle", RUNNING.has(phase)]);

  // Terminal phases: success selects the project; failure and cancel return
  // to the form with every value still in place.
  useEffect(() => {
    if (!snap) return;
    const result = snap.phase;
    if (result.kind === "succeeded" && !clonedReported.current) {
      clonedReported.current = true;
      onCloned?.({ name: result.name, dest: result.dest });
    } else if (result.kind === "failed") {
      setFailure({ message: result.message, advice: result.advice, fixes: result.fixes });
      setSnap(null);
      void cloneProjectReset().catch(() => undefined);
    } else if (result.kind === "cancelled") {
      setSnap(null);
      void cloneProjectReset().catch(() => undefined);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [snap]);

  async function runPreflight() {
    setBusy(true);
    setCloneError(null);
    setFailure(null);
    try {
      setSnap(await cloneProjectPreflight(url.trim(), dest.trim(), name.trim()));
    } catch (cause) {
      setCloneError(asGuiError(cause).message);
    } finally {
      setBusy(false);
    }
  }

  async function startClone() {
    setBusy(true);
    setCloneError(null);
    try {
      setSnap(await cloneProjectStart(url.trim(), dest.trim(), name.trim()));
    } catch (cause) {
      setCloneError(asGuiError(cause).message);
    } finally {
      setBusy(false);
    }
  }

  async function backToForm() {
    try { await cloneProjectReset(); } catch { /* still running: the form stays on its flow view */ return; }
    setSnap(null);
    setCloneError(null);
  }

  async function close() {
    if (RUNNING.has(phase)) {
      if (phase === "registering" || (phase === "cloning" && !snap?.can_cancel)) return;
      try { await cloneProjectCancel(); } catch { /* the reset below reports nothing more */ }
    }
    if (snap) await cloneProjectReset().catch(() => undefined);
    onCancel();
  }

  function flowBody() {
    const result = snap?.phase;
    return (
      <div className="form-stack">
        <p className="field-hint mono">{snap?.url}</p>
        {result?.kind === "preflight_running" && (
          <div className="clone-progress" role="status"><Spinner /> Checking that the repository can be reached…</div>
        )}
        {result?.kind === "preflight_done" && (
          <PreflightChecklist checks={result.checks} defaultBranch={result.default_branch} />
        )}
        {result?.kind === "cloning" && <CloneProgressView progress={result.progress} />}
        {result?.kind === "registering" && <div className="clone-progress" role="status"><Spinner /> Adding the project…</div>}
        {result?.kind === "succeeded" && <p role="status">Cloned {result.name} on {result.branch}.</p>}
        {cloneError && <p className="field-error" role="alert">{cloneError}</p>}
      </div>
    );
  }

  function flowFooter() {
    const result = snap?.phase;
    if (result?.kind === "preflight_running" || result?.kind === "cloning" || result?.kind === "registering") {
      return (
        <button type="button" className="btn btn-ghost" disabled={!snap?.can_cancel}
          onClick={() => void cloneProjectCancel().then(setSnap, (cause) => setCloneError(asGuiError(cause).message))}>
          Cancel
        </button>
      );
    }
    return (
      <>
        <button type="button" className="btn btn-ghost" disabled={busy} onClick={() => void backToForm()}>Back</button>
        <button type="button" className="btn" disabled={busy} onClick={() => void runPreflight()}>Re-check</button>
        <button type="button" className="btn btn-primary" disabled={busy || !snap?.can_start} onClick={() => void startClone()}>
          Start clone
        </button>
      </>
    );
  }

  return (
    <Modal
      label="New project"
      title="New project"
      subtitle="A project is a git repository on this computer. AMF runs agents in it, one feature at a time."
      onClose={() => void close()}
      dismissable={phase !== "registering"}
      onSubmit={inFlow ? undefined : () => {
        if (cloneMode) { void runPreflight(); return; }
        onSubmit({
          path: trimmed,
          project_name: name.trim(),
          preferred_agent: selectedAgent || null,
          dry_run: false,
        });
      }}
      footer={inFlow ? flowFooter() : (
        <>
          <button type="button" className="btn btn-ghost" onClick={() => void close()}>Cancel</button>
          <button type="submit" className="btn btn-primary"
            disabled={cloneMode ? busy || !!urlError || !name.trim() || !dest.trim() : pending}>
            {(cloneMode ? busy : pending) && <Spinner />}
            {cloneMode ? (busy ? "Checking…" : "Check and clone") : pending ? "Creating…" : "Create project"}
          </button>
        </>
      )}
    >
      {inFlow ? flowBody() : (
      <div className="form-stack">
        <Field label="Git URL (optional)" hint={urlError
          ? <span className="field-error" role="alert">{urlError}</span>
          : "Paste an HTTPS or SSH URL to clone it as a new project. Leave empty to add a folder already on this computer."}>
          <input
            className="mono"
            value={url}
            onChange={(event) => { setUrl(event.target.value); setCloneError(null); setFailure(null); onEdit(); }}
            placeholder="https://github.com/owner/repo.git"
            aria-invalid={urlError ? true : undefined}
            spellCheck={false}
          />
        </Field>
        {cloneMode && failure && <FailureNotice {...failure} />}
        {cloneMode && cloneError && (
          <p className="field-error" role="alert"><strong>Could not start.</strong> {cloneError}</p>
        )}
        {cloneMode ? (
          <>
            <Field label="Project name">
              <input value={name} required onChange={(event) => {
                setName(event.target.value); setNameEdited(true); setCloneError(null);
              }} />
            </Field>
            <Field label="Clone into" hint="The folder is created for the clone; an existing folder is never reused.">
              <input className="mono" value={dest} required onChange={(event) => {
                setDest(event.target.value); setDestEdited(true); setCloneError(null);
              }} />
            </Field>
          </>
        ) : (<>
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
        </>)}
      </div>
      )}
    </Modal>
  );
}
