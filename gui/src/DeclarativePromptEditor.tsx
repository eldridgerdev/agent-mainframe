import { useRef, useState } from "react";
import { asGuiError } from "./api";
import { DeclarativeEditorData, promptFileReload, promptFileSave } from "./promptFilesApi";
import { Field, Modal, Spinner } from "./ui";

export default function DeclarativePromptEditor({ snapshot, onClose, onSaved }: {
  snapshot: DeclarativeEditorData;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [original, setOriginal] = useState(snapshot);
  const [name, setName] = useState(snapshot.name ?? "");
  const [body, setBody] = useState(snapshot.body ?? "");
  const [tags, setTags] = useState(snapshot.tags ?? "");
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const [error, setError] = useState<string | null>(null);
  const [discard, setDiscard] = useState<"close" | "reload" | null>(null);
  const dirty = name !== (original?.name ?? "") || body !== (original?.body ?? "") || tags !== (original?.tags ?? "");
  function close() { if (busy || inFlight.current) return; if (dirty) setDiscard("close"); else onClose(); }
  async function reload() {
    if (inFlight.current || !original) return;
    inFlight.current = true; setBusy(true); setError(null);
    try {
      const fresh = await promptFileReload(original);
      setOriginal(fresh); setName(fresh.name); setBody(fresh.body); setTags(fresh.tags); setDiscard(null);
    } catch (err) { setError(asGuiError(err).message); setDiscard(null); }
    finally { inFlight.current = false; setBusy(false); }
  }
  return <>
    <Modal label="Declarative prompt editor" title="Edit declarative prompt" size="lg"
      subtitle="Edit this template in its original configuration scope. Other metadata is kept."
      hidden={discard !== null} dismissable={!busy} onClose={close}
      onSubmit={async () => {
        if (inFlight.current || !name.trim() || !body.trim()) return;
        inFlight.current = true; setBusy(true); setError(null);
        try {
          await promptFileSave(original, name, body, tags);
          onSaved();
        } catch (err) { setError(asGuiError(err).message); }
        finally { inFlight.current = false; setBusy(false); }
      }}
      footer={<>
        {original && <button type="button" className="btn btn-secondary" disabled={busy}
          onClick={() => { if(inFlight.current)return; if(dirty)setDiscard("reload");else void reload(); }}>Reload template</button>}
        <button type="button" className="btn btn-ghost" disabled={busy} onClick={close}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={busy || !name.trim() || !body.trim()}>
          {busy && <Spinner />}Save template
        </button>
      </>}>
      <p className="muted small">Write to: <code>{original.destination_path}</code></p>
      {original.source_path !== original.destination_path && <p className="muted small">Read from: <code>{original.source_path}</code>. The legacy file is kept.</p>}
      {error && <p role="alert" className="callout callout-danger">{error}</p>}
      {original?.description && <p className="muted">Description (kept): {original.description}</p>}
      {!!original?.configured_fields && <p className="muted small">{original.configured_fields} configured placeholder definitions are kept, including ones not used in the body.</p>}
      <fieldset className="form-stack" disabled={busy} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>
        <Field label="Template name"><input value={name} onChange={(event) => setName(event.target.value)} autoFocus required /></Field>
        <Field label="Tags" hint="Separate tags with commas or spaces."><input value={tags} onChange={(event) => setTags(event.target.value)} /></Field>
        <Field label="Template body" hint="Use {{placeholders}} to fill values when adding the template to a draft.">
          <textarea value={body} onChange={(event) => setBody(event.target.value)} rows={12} required />
        </Field>
      </fieldset>
    </Modal>
    {discard && <Modal label="Discard prompt edits" title={discard === "reload" ? "Discard edits and reload this prompt?" : "Discard unsaved prompt edits?"}
      dismissable={!busy} onClose={() => { if (!inFlight.current && !busy) setDiscard(null); }}
      footer={<><button type="button" className="btn btn-secondary" disabled={busy} onClick={() => { if (!inFlight.current) setDiscard(null); }}>Keep editing</button>
        <button type="button" className="btn btn-danger" disabled={busy} onClick={() => discard === "reload" ? void reload() : onClose()}>Discard changes</button></>}>
      <p>Your edited template will be lost. The original composer draft is kept.</p>
    </Modal>}
  </>;
}
