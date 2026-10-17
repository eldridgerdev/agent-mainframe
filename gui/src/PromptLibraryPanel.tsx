import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  LibraryEntry, LibraryScope, Project, SessionTarget, asGuiError,
  UserPromptEditorData, promptLibraryLoad, promptLibraryResolve, promptUserEdit, promptUserDelete, promptUserDuplicate,
} from "./api";
import UserPromptEditor from "./UserPromptEditor";
import DeclarativePromptEditor from "./DeclarativePromptEditor";
import PromptExportDialog from "./PromptExportDialog";
import { DeclarativeEditorData, promptFileEdit } from "./promptFilesApi";
import { renderTemplate } from "./promptTemplate";
import { EmptyState, Field, Modal, Spinner } from "./ui";

const targetKey = (target: SessionTarget) => JSON.stringify([target.project_id, target.feature_id, target.session_id]);

export default function PromptLibraryPanel({ initialScope, initialTarget, projects, onClose, onInsert }: {
  initialScope: LibraryScope;
  initialTarget: SessionTarget | null;
  projects: Project[];
  onClose: () => void;
  onInsert: (target: SessionTarget, text: string) => void;
}) {
  const queryClient = useQueryClient();
  const [editor, setEditor] = useState<{ snapshot: UserPromptEditorData | null } | null>(null);
  const [fileEditor,setFileEditor] = useState<DeclarativeEditorData|null>(null);
  const [exporting,setExporting] = useState<LibraryEntry|null>(null);
  const [deleting, setDeleting] = useState<UserPromptEditorData | null>(null);
  const [authorBusy, setAuthorBusy] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [scope, setScope] = useState(initialScope);
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<LibraryEntry | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const [destination, setDestination] = useState(initialTarget ? targetKey(initialTarget) : "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const inFlight = useRef(false);
  const authorPending = useRef(false);
  const active = useRef(true);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  const library = useQuery({
    queryKey: ["prompt-library", scope, search],
    queryFn: () => promptLibraryLoad(scope, search),
    retry: false, staleTime: 0, refetchInterval: busy || authorBusy || editor || deleting || fileEditor || exporting ? false : 3000,
  });
  // A key is the entry's content, so an external edit or deletion retires it.
  // `available_keys` ignores the search, which only hides entries.
  useEffect(() => {
    if (selected && library.data && !library.data.available_keys.includes(selected.key)) {
      setSelected(null); setValues({}); setError(null);
      setNotice(`“${selected.name}” was changed or removed outside this window. Select it again to use the current version.`);
    }
  }, [library.data, selected]);
  const pairs: [string, string][] = selected?.slots.map((slot) => [slot.key, values[slot.key] ?? ""]) ?? [];
  // Rendered locally: the backend re-resolves (and revalidates) only on insertion.
  const preview = selected ? renderTemplate(selected.body, pairs) : "";
  const target = library.data?.targets.find((candidate) => targetKey(candidate.target) === destination);
  const missingRequired = selected?.slots.some((slot) => slot.required && !(values[slot.key] ?? "").trim());
  const scopes: { label: string; value: LibraryScope }[] = [{ label: "User & global prompts", value: { kind: "global" } }];
  for (const project of projects) {
    scopes.push({ label: project.name, value: { kind: "project", project_id: project.id } });
    for (const feature of project.features) scopes.push({
      label: `${project.name} / ${feature.name}`,
      value: { kind: "feature", project_id: project.id, feature_id: feature.id },
    });
  }
  function choose(entry: LibraryEntry) {
    setSelected(entry);
    setValues(Object.fromEntries(entry.slots.map((slot) => [slot.key, slot.initial_value])));
    setError(null); setNotice(null);
  }
  function close() {
    if (authorPending.current || authorBusy || editor || deleting || fileEditor || exporting) return;
    active.current = false;
    onClose();
  }
  async function insert() {
    if (inFlight.current || !selected || !target || missingRequired) return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    let inserted = false;
    try {
      const text = await promptLibraryResolve({ scope, entry_key: selected.key, values: pairs, target: target.target });
      if (active.current) { onInsert(target.target, text); inserted = true; }
    } catch (err) {
      if (active.current) { setError(asGuiError(err).message); void library.refetch(); }
    } finally {
      if (!inserted) {
        inFlight.current = false;
        if (active.current) setBusy(false);
      }
    }
  }
  function saved() {
    setEditor(null); setSelected(null); setValues({}); setError(null); setSearch("");
    setNotice("Saved in your User library. Your composer draft is unchanged.");
    void queryClient.invalidateQueries({ queryKey: ["prompt-library"] });
  }
  function fileSaved() {
    setFileEditor(null);setExporting(null);setSelected(null);setValues({});setSearch("");setError(null);
    setNotice("Saved configuration template. Your composer draft is unchanged.");
    void queryClient.invalidateQueries({queryKey:["prompt-library"]});
  }
  async function editFile() {
    if(inFlight.current||!selected||library.isError)return;
    inFlight.current=true;authorPending.current=true;setAuthorBusy(true);setError(null);
    try{setFileEditor(await promptFileEdit(scope,selected.key));}
    catch(err){setError(asGuiError(err).message);}
    finally{inFlight.current=false;authorPending.current=false;setAuthorBusy(false);}
  }
  async function author(action: "edit" | "delete" | "duplicate") {
    if (inFlight.current || !selected || library.isError || authorBusy) return;
    inFlight.current = true; authorPending.current = true; setAuthorBusy(true); setError(null);
    try {
      if (action === "duplicate") {
        await promptUserDuplicate(scope, selected.key);
        setSelected(null); setValues({}); setSearch(""); setNotice("Duplicated to your User library.");
        await queryClient.invalidateQueries({ queryKey: ["prompt-library"] });
      } else {
        const snapshot = await promptUserEdit(scope, selected.key);
        if (action === "edit") setEditor({ snapshot });
        else { setDeleting(snapshot); setDeleteError(null); }
      }
    } catch (err) { setError(asGuiError(err).message); }
    finally { inFlight.current = false; authorPending.current = false; setAuthorBusy(false); }
  }
  return <><Modal label="Prompt library" title="Prompt library" size="xl" onClose={close} hidden={!!editor || !!deleting || !!fileEditor || !!exporting} dismissable={!authorBusy}
    subtitle="Choose a saved prompt, fill its fields, then add it to an agent draft."
    footer={<>
      <span className="muted small">Existing draft text is kept. Send explicitly from the composer.</span>
      <button className="btn btn-secondary" disabled={authorBusy} onClick={close}>Cancel</button>
      <button className="btn btn-primary" onClick={() => void insert()}
        disabled={busy || authorBusy || !selected || !target || missingRequired || library.isError || !preview.trim()}>
        {busy && <Spinner />}{busy ? "Adding…" : "Add to draft"}
      </button>
    </>}>
    <div className="library-toolbar">
      <button className="btn btn-primary btn-sm" disabled={busy || authorBusy} onClick={() => setEditor({ snapshot: null })}>New User template</button>
      <Field label="Library scope"><select disabled={busy || authorBusy} value={JSON.stringify(scope)} onChange={(event) => {
        setScope(JSON.parse(event.target.value) as LibraryScope); setSelected(null); setValues({}); setError(null); setNotice(null);
      }}>
        {!scopes.some((candidate) => JSON.stringify(candidate.value) === JSON.stringify(scope)) &&
          <option value={JSON.stringify(scope)}>Scope no longer available</option>}
        {scopes.map((candidate) => <option key={JSON.stringify(candidate.value)} value={JSON.stringify(candidate.value)}>{candidate.label}</option>)}
      </select></Field>
      <Field label="Search prompts"><input autoFocus disabled={busy || authorBusy} placeholder="Search name, body or #tag…" value={search}
        onChange={(event) => setSearch(event.target.value)} /></Field>
      <button className="btn btn-secondary btn-sm" disabled={library.isFetching || busy || authorBusy} onClick={() => void library.refetch()}>Refresh</button>
    </div>
    {authorBusy && <p role="status" className="muted"><Spinner /> Updating User library…</p>}
    {library.error && <p role="alert" className="error-text">{asGuiError(library.error).message}</p>}
    <div className="library-browser">
      <div className="library-list" role="group" aria-label="Prompt library entries">
        {library.isLoading && <p className="muted">Loading prompts…</p>}
        {library.data?.entries.length === 0 && <EmptyState icon="file" title={search ? "No matching prompts" : "No saved prompts"}>
          {search ? "Try a different search or library scope." : "Create a User template here, or configure declarative templates in amf.json."}
        </EmptyState>}
        {library.data?.entries.map((entry) => <button key={entry.key} disabled={busy || authorBusy}
          className={`library-item ${selected?.key === entry.key ? "library-item-active" : ""}`}
          aria-pressed={selected?.key === entry.key} onClick={() => choose(entry)}>
          <span className="library-item-heading"><strong>{entry.name}</strong><span className="badge">{entry.source}</span></span>
          {entry.description && <span className="muted small">{entry.description}</span>}
          {entry.tags.length > 0 && <span className="muted small">{entry.tags.map((tag) => `#${tag}`).join(" ")}</span>}
        </button>)}
      </div>
      <div className="library-detail">
        {!selected && notice && <p role="status" className="muted">{notice}</p>}
        {!selected && <EmptyState icon="file" title="Select a prompt">Preview its text and customize any placeholders before adding it.</EmptyState>}
        {selected && <>
          <div className="library-actions">
            <button className="btn btn-secondary btn-sm" disabled={busy || authorBusy || library.isError} onClick={() => void author("duplicate")}>Duplicate to User</button>
            {selected.source === "User" && <>
              <button className="btn btn-secondary btn-sm" disabled={busy || authorBusy || library.isError} onClick={() => void author("edit")}>Edit User template</button>
              <button className="btn btn-ghost btn-sm" disabled={busy || authorBusy || library.isError} onClick={() => void author("delete")}>Delete User template…</button>
            </>}
            {selected.source !== "User" && <button className="btn btn-secondary btn-sm" disabled={busy||authorBusy||library.isError} onClick={()=>void editFile()}>Edit declarative template</button>}
            <button className="btn btn-secondary btn-sm" disabled={busy||authorBusy||library.isError} onClick={()=>setExporting(selected)}>Export template…</button>
          </div>
          <h3>{selected.name} <span className="badge">{selected.source}</span></h3>
          {selected.description && <p className="muted">{selected.description}</p>}
          <details><summary>Original template</summary><pre className="library-preview">{selected.body}</pre></details>
          {selected.slots.length > 0 && <div className="library-fields">
            {selected.slots.map((slot) => <Field key={slot.key} label={`${slot.label}${slot.required ? " (required)" : ""}`}>
              {slot.kind === "select" ? <select disabled={busy || authorBusy} aria-required={slot.required} value={values[slot.key] ?? ""}
                onChange={(event) => setValues((current) => ({ ...current, [slot.key]: event.target.value }))}>
                {slot.options.length === 0 && <option value="">No configured choices</option>}
                {slot.options.map((option, index) => <option key={index} value={option}>{option}</option>)}
              </select> : slot.kind === "multi_line" ? <textarea disabled={busy || authorBusy} aria-required={slot.required} rows={3} value={values[slot.key] ?? ""}
                onChange={(event) => setValues((current) => ({ ...current, [slot.key]: event.target.value }))} /> :
                <input disabled={busy || authorBusy} aria-required={slot.required} value={values[slot.key] ?? ""}
                  onChange={(event) => setValues((current) => ({ ...current, [slot.key]: event.target.value }))} />}
            </Field>)}
          </div>}
          <h4>Prompt preview</h4>
          <pre className="library-preview" aria-label="Resolved prompt">{preview}</pre>
          <Field label="Agent session"><select disabled={busy || authorBusy} value={destination} onChange={(event) => { setDestination(event.target.value); setError(null); }}>
            <option value="">Choose an agent session…</option>
            {destination && !target && <option value={destination}>Selected session no longer available</option>}
            {library.data?.targets.map((candidate) => <option key={targetKey(candidate.target)} value={targetKey(candidate.target)}>
              {candidate.label}{candidate.stopped ? " (stopped)" : ""}
            </option>)}
          </select></Field>
          {library.data?.targets.length === 0 && <p className="muted small">Add an allowed agent session to a feature to use this prompt.</p>}
          {target?.stopped && <p className="muted small">You can prepare this draft now. Start the session when you’re ready to send.</p>}
          {missingRequired && <p className="muted small">Fill the required fields before adding this prompt.</p>}
          {error && <p role="alert" className="error-text">{error}</p>}
        </>}
      </div>
    </div>
  </Modal>
  {fileEditor && <DeclarativePromptEditor snapshot={fileEditor} onClose={()=>setFileEditor(null)} onSaved={fileSaved}/> }
  {exporting && <PromptExportDialog scope={scope} entry={exporting} projects={projects} onClose={()=>setExporting(null)} onSaved={fileSaved}/> }
  {editor && <UserPromptEditor snapshot={editor.snapshot} onClose={() => setEditor(null)} onSaved={saved} />}
  {deleting && <Modal label="Delete User template" title="Delete this User template?" onClose={() => { if (!inFlight.current && !authorBusy) setDeleting(null); }} dismissable={!authorBusy}
    footer={<><button className="btn btn-secondary" disabled={authorBusy} onClick={() => { if (!inFlight.current) setDeleting(null); }}>Cancel</button>
      <button className="btn btn-danger" disabled={authorBusy} onClick={async () => {
        if (inFlight.current) return;
        inFlight.current = true; authorPending.current = true; setAuthorBusy(true); setDeleteError(null);
        try {
          await promptUserDelete(deleting.id, deleting.revision);
          setDeleting(null); setSelected(null); setValues({}); setNotice("Deleted from your User library.");
          await queryClient.invalidateQueries({ queryKey: ["prompt-library"] });
        } catch (err) { setDeleteError(asGuiError(err).message); }
        finally { inFlight.current = false; authorPending.current = false; setAuthorBusy(false); }
      }}>{authorBusy && <Spinner />}Delete User template</button></>}>
    <p>Delete “{deleting.name}”? Existing composer drafts and declarative templates are kept.</p>
    {deleteError && <p role="alert" className="callout callout-danger">{deleteError}</p>}
  </Modal>}
  </>;
}
