import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  LibraryEntry, LibraryScope, Project, SessionTarget, asGuiError,
  promptLibraryLoad, promptLibraryResolve,
} from "./api";
import { EmptyState, Field, Modal, Spinner } from "./ui";

const targetKey = (target: SessionTarget) => JSON.stringify([target.project_id, target.feature_id, target.session_id]);

export default function PromptLibraryPanel({ initialScope, initialTarget, projects, onClose, onInsert }: {
  initialScope: LibraryScope;
  initialTarget: SessionTarget | null;
  projects: Project[];
  onClose: () => void;
  onInsert: (target: SessionTarget, text: string) => void;
}) {
  const [scope, setScope] = useState(initialScope);
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<LibraryEntry | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const [destination, setDestination] = useState(initialTarget ? targetKey(initialTarget) : "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const active = useRef(true);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  const library = useQuery({
    queryKey: ["prompt-library", scope, search],
    queryFn: () => promptLibraryLoad(scope, search),
    retry: false, staleTime: 0, refetchInterval: busy ? false : 3000,
  });
  const pairs: [string, string][] = selected?.slots.map((slot) => [slot.key, values[slot.key] ?? ""]) ?? [];
  const preview = useQuery({
    queryKey: ["prompt-library-preview", scope, selected?.key, pairs],
    queryFn: () => promptLibraryResolve({ scope, entry_key: selected!.key, values: pairs, target: null }),
    enabled: selected !== null, retry: false, staleTime: 0,
  });
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
    setError(null);
  }
  function close() {
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
  return <Modal label="Prompt library" title="Prompt library" size="xl" onClose={close}
    subtitle="Choose a saved prompt, fill its fields, then add it to an agent draft."
    footer={<>
      <span className="muted small">Existing draft text is kept. Send explicitly from the composer.</span>
      <button className="btn btn-secondary" onClick={close}>Cancel</button>
      <button className="btn btn-primary" onClick={() => void insert()}
        disabled={busy || !selected || !target || missingRequired || library.isError || preview.isFetching || preview.isError || !preview.data?.trim()}>
        {busy && <Spinner />}{busy ? "Adding…" : "Add to draft"}
      </button>
    </>}>
    <div className="library-toolbar">
      <Field label="Library scope"><select disabled={busy} value={JSON.stringify(scope)} onChange={(event) => {
        setScope(JSON.parse(event.target.value) as LibraryScope); setSelected(null); setValues({}); setError(null);
      }}>
        {!scopes.some((candidate) => JSON.stringify(candidate.value) === JSON.stringify(scope)) &&
          <option value={JSON.stringify(scope)}>Scope no longer available</option>}
        {scopes.map((candidate) => <option key={JSON.stringify(candidate.value)} value={JSON.stringify(candidate.value)}>{candidate.label}</option>)}
      </select></Field>
      <Field label="Search prompts"><input autoFocus disabled={busy} placeholder="Search name, body or #tag…" value={search}
        onChange={(event) => setSearch(event.target.value)} /></Field>
      <button className="btn btn-secondary btn-sm" disabled={library.isFetching || busy} onClick={() => { void library.refetch(); if (selected) void preview.refetch(); }}>Refresh</button>
    </div>
    {library.error && <p role="alert" className="error-text">{asGuiError(library.error).message}</p>}
    <div className="library-browser">
      <div className="library-list" role="group" aria-label="Prompt library entries">
        {library.isLoading && <p className="muted">Loading prompts…</p>}
        {library.data?.entries.length === 0 && <EmptyState icon="file" title={search ? "No matching prompts" : "No saved prompts"}>
          {search ? "Try a different search or library scope." : "Save prompts in the TUI or configure templates in amf.json to use them here."}
        </EmptyState>}
        {library.data?.entries.map((entry) => <button key={entry.key} disabled={busy}
          className={`library-item ${selected?.key === entry.key ? "library-item-active" : ""}`}
          aria-pressed={selected?.key === entry.key} onClick={() => choose(entry)}>
          <span className="library-item-heading"><strong>{entry.name}</strong><span className="badge">{entry.source}</span></span>
          {entry.description && <span className="muted small">{entry.description}</span>}
          {entry.tags.length > 0 && <span className="muted small">{entry.tags.map((tag) => `#${tag}`).join(" ")}</span>}
        </button>)}
      </div>
      <div className="library-detail">
        {!selected && <EmptyState icon="file" title="Select a prompt">Preview its text and customize any placeholders before adding it.</EmptyState>}
        {selected && <>
          <h3>{selected.name} <span className="badge">{selected.source}</span></h3>
          {selected.description && <p className="muted">{selected.description}</p>}
          <details><summary>Original template</summary><pre className="library-preview">{selected.body}</pre></details>
          {selected.slots.length > 0 && <div className="library-fields">
            {selected.slots.map((slot) => <Field key={slot.key} label={`${slot.label}${slot.required ? " (required)" : ""}`}>
              {slot.kind === "select" ? <select disabled={busy} aria-required={slot.required} value={values[slot.key] ?? ""}
                onChange={(event) => setValues((current) => ({ ...current, [slot.key]: event.target.value }))}>
                {slot.options.length === 0 && <option value="">No configured choices</option>}
                {slot.options.map((option, index) => <option key={index} value={option}>{option}</option>)}
              </select> : slot.kind === "multi_line" ? <textarea disabled={busy} aria-required={slot.required} rows={3} value={values[slot.key] ?? ""}
                onChange={(event) => setValues((current) => ({ ...current, [slot.key]: event.target.value }))} /> :
                <input disabled={busy} aria-required={slot.required} value={values[slot.key] ?? ""}
                  onChange={(event) => setValues((current) => ({ ...current, [slot.key]: event.target.value }))} />}
            </Field>)}
          </div>}
          <h4>Prompt preview</h4>
          {preview.isFetching && <p className="muted small">Updating preview…</p>}
          {preview.error && <p role="alert" className="error-text">{asGuiError(preview.error).message}</p>}
          {!preview.isError && <pre className="library-preview" aria-label="Resolved prompt">{preview.data ?? ""}</pre>}
          <Field label="Agent session"><select disabled={busy} value={destination} onChange={(event) => { setDestination(event.target.value); setError(null); }}>
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
  </Modal>;
}
