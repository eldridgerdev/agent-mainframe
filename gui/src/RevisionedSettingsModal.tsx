import { ReactNode, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { asGuiError } from "./api";
import { Modal, Spinner } from "./ui";

/** One scoped slice of a config file, with the revision it was read at. The
 * backend refuses a save made against an older revision. */
export interface RevisionedView<S> {
  settings: S;
  revision: string;
}

/** The modal around a revisioned settings form: load on open, save against
 * the loaded revision, reload, and ask before discarding a dirty draft. One
 * request runs at a time, and nothing can be dismissed, discarded or re-sent
 * while it does. A panel supplies only its fields. */
export default function RevisionedSettingsModal<S, V extends RevisionedView<S> = RevisionedView<S>>({
  label, subtitle, loadCommand, saveCommand, args, onLoaded, draft, isDirty, onClose, onSaved, children,
}: {
  label: string;
  /** May depend on what was loaded; `null` until then. */
  subtitle: string | ((view: V | null) => string);
  loadCommand: string;
  saveCommand: string;
  /** What scopes both commands, such as a project id. */
  args?: Record<string, unknown>;
  /** Reset the panel's fields from a freshly loaded view. */
  onLoaded: (view: V) => void;
  /** What Save would send, or `null` while the fields are invalid. */
  draft: S | null;
  /** Whether the fields differ from the loaded settings. */
  isDirty: (loaded: S) => boolean;
  onClose: () => void;
  onSaved: () => void;
  /** The fields, shown once settings have loaded. */
  children: (busy: boolean, view: V) => ReactNode;
}) {
  const [view, setView] = useState<V | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [discard, setDiscard] = useState<"close" | "reload" | null>(null);
  const inFlight = useRef(false);
  const dirty = !!view && isDirty(view.settings);

  async function load() {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true); setError(null);
    try {
      const fresh = await (args ? invoke<V>(loadCommand, args) : invoke<V>(loadCommand));
      setView(fresh);
      onLoaded(fresh);
    } catch (err) { setError(asGuiError(err).message); }
    finally { inFlight.current = false; setBusy(false); }
  }
  useEffect(() => { void load(); }, []);

  function request(action: "close" | "reload") {
    if (inFlight.current) return;
    if (dirty) setDiscard(action);
    else if (action === "close") onClose();
    else void load();
  }
  async function save() {
    if (inFlight.current || !view || draft === null) return;
    inFlight.current = true;
    setBusy(true); setError(null);
    try {
      await invoke<V>(saveCommand, { ...args, revision: view.revision, settings: draft });
      onSaved();
    } catch (err) { setError(asGuiError(err).message); }
    finally { inFlight.current = false; setBusy(false); }
  }

  return <Modal label={label} title={label} onClose={() => request("close")}
    dismissable={!busy} subtitle={typeof subtitle === "function" ? subtitle(view) : subtitle}
    footer={<>
      <button className="btn btn-ghost" disabled={busy} onClick={() => request("close")}>Cancel</button>
      <button className="btn btn-secondary" disabled={busy} onClick={() => request("reload")}>Reload settings</button>
      <button className="btn btn-primary" disabled={busy || !view || !dirty || draft === null} onClick={() => void save()}>Save settings</button>
    </>}>
    {busy && <p role="status"><Spinner /> Loading or saving settings…</p>}
    {error && <p className="callout callout-danger" role="alert">{error}</p>}
    {view && <div className="form-stack">{children(busy, view)}</div>}
    {discard && <div role="alertdialog" aria-label="Discard unsaved settings" className="callout callout-warning">
      <p>Discard unsaved settings and {discard === "close" ? "close" : "reload"}?</p>
      <button className="btn btn-secondary" disabled={busy} onClick={() => {
        if (inFlight.current) return;
        setDiscard(null);
      }}>Keep editing</button>
      <button className="btn btn-warning" disabled={busy} onClick={() => {
        if (inFlight.current) return;
        const action = discard; setDiscard(null);
        if (action === "close") onClose(); else void load();
      }}>Discard changes</button>
    </div>}
  </Modal>;
}
