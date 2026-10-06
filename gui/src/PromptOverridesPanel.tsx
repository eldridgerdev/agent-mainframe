import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AgentSlug, Project, asGuiError } from "./api";
import {
  HARNESS_LABELS, OverrideContext, OverrideRow, OverrideScope, OverridesView, StoredOverride,
  promptOverridesClear, promptOverridesLoad, promptOverridesSave, slotLabel,
} from "./promptOverridesApi";
import { EmptyState, Field, Modal, Spinner } from "./ui";

const HARNESSES: AgentSlug[] = ["claude", "opencode", "codex", "pi"];
const contextKey = (context: OverrideContext) => JSON.stringify(context);

/** A local, unsaved template edit for one prompt in one context. */
interface Draft {
  promptId: string;
  context: OverrideContext;
  scope: OverrideScope;
  harness: AgentSlug | null;
  text: string;
  initialText: string;
  /** The prompt's revision this draft is based on; saves send it back. */
  revision: string;
}

/** A pending clear: the slot and the prompt revision the user confirmed against. */
interface PendingClear {
  promptId: string;
  slot: StoredOverride;
  /** Captured when the confirmation opened, not re-read from the poll. */
  revision: string;
}

/**
 * The headless-prompt override manager (the TUI's dashboard `E`). Reads and
 * writes go through the shared registry, resolver and stores; every write is
 * checked against the revision the draft was based on, so a change made by the
 * TUI, another window or a hand edit of `amf.json` must be reloaded first.
 */
export default function PromptOverridesPanel({ initialContext, initialPromptId, initialHarness, fromPrecall, projects, onClose }: {
  initialContext: OverrideContext;
  initialPromptId: string | null;
  initialHarness: AgentSlug | null;
  /** Opened from a pending AI call's notice, which stays open underneath. */
  fromPrecall: boolean;
  projects: Project[];
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [context, setContext] = useState(initialContext);
  const [harness, setHarness] = useState<AgentSlug | null>(initialHarness);
  const [selectedId, setSelectedId] = useState<string | null>(initialPromptId);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [confirmClear, setConfirmClear] = useState<PendingClear | null>(null);
  const [pendingDiscard, setPendingDiscard] = useState<{ label: string; run: () => void } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const inFlight = useRef(false);
  const activeItem = useRef<HTMLButtonElement | null>(null);
  const queryKey = ["prompt-overrides", contextKey(context), harness];
  const overrides = useQuery({
    queryKey,
    queryFn: () => promptOverridesLoad(context, harness),
    retry: false, staleTime: 0, refetchInterval: busy ? false : 4000,
  });
  const view = overrides.data;
  const row = view?.rows.find((candidate) => candidate.id === selectedId) ?? view?.rows[0] ?? null;
  useEffect(() => {
    if (!selectedId && view?.rows[0]) setSelectedId(view.rows[0].id);
  }, [selectedId, view]);
  // Keep a focused prompt (for example one opened from a pre-call notice) in view.
  useEffect(() => { activeItem.current?.scrollIntoView?.({ block: "nearest" }); }, [row?.id]);
  const draftRow = draft && view && contextKey(view.context) === contextKey(draft.context)
    ? view.rows.find((candidate) => candidate.id === draft.promptId) ?? null : null;
  const dirty = draft !== null && draft.text !== draft.initialText;
  const stale = draft !== null && draftRow !== null && draftRow.revision !== draft.revision;
  const clearStale = confirmClear !== null && row !== null && (row.id !== confirmClear.promptId || row.revision !== confirmClear.revision);

  function guard(label: string, run: () => void) {
    if (dirty) setPendingDiscard({ label, run });
    else { setDraft(null); run(); }
  }
  function requestClose() {
    if (busy) return;
    guard("close the manager", onClose);
  }
  // The manager can sit on top of Final Review, whose modal also closes on
  // Escape: claim the key first so one press only ever reaches this dialog.
  const closeRef = useRef(requestClose);
  closeRef.current = requestClose;
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      closeRef.current();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  const contexts: { label: string; value: OverrideContext }[] = [{ label: "Global (all projects)", value: { kind: "global" } }];
  for (const project of projects) {
    contexts.push({ label: project.name, value: { kind: "project", project_id: project.id } });
    for (const feature of project.features) contexts.push({
      label: `${project.name} / ${feature.name}`,
      value: { kind: "feature", project_id: project.id, feature_id: feature.id },
    });
  }
  const availableScopes = view?.scopes.filter((scope) => scope.available) ?? [];

  function startEdit(target: OverrideRow, slot: StoredOverride | null) {
    if (!view) return;
    const scope = slot?.scope ?? availableScopes[0]?.scope;
    if (!scope) { setError("No scope is available for a new override here."); return; }
    const text = slot?.template ?? target.effective_template;
    setDraft({
      promptId: target.id, context: view.context, scope, harness: slot?.harness ?? null,
      text, initialText: text, revision: target.revision,
    });
    setConfirmClear(null); setError(null); setNotice(null);
  }
  function reloadDraftBase() {
    if (!draft || !draftRow) return;
    setDraft({ ...draft, revision: draftRow.revision });
    setError(null);
  }
  async function run(action: () => Promise<OverridesView>, done: string) {
    if (inFlight.current) return false;
    inFlight.current = true; setBusy(true); setError(null); setNotice(null);
    try {
      const next = await action();
      queryClient.setQueryData(["prompt-overrides", contextKey(next.context), harness], next);
      setNotice(done);
      return true;
    } catch (err) {
      setError(asGuiError(err).message);
      void overrides.refetch();
      return false;
    } finally {
      inFlight.current = false; setBusy(false);
    }
  }
  async function save() {
    if (!draft || stale || !draft.text.trim()) return;
    const saved = await run(() => promptOverridesSave({
      context: draft.context, prompt_id: draft.promptId, scope: draft.scope, harness: draft.harness,
      template: draft.text, revision: draft.revision, view_harness: harness,
    }), `Saved the ${slotLabel(draft.scope, draft.harness)} override for ${draft.promptId}.`);
    if (saved) setDraft(null);
  }
  async function clear(pending: PendingClear) {
    if (!view || clearStale) return;
    const { promptId, slot, revision } = pending;
    const cleared = await run(() => promptOverridesClear({
      context: view.context, prompt_id: promptId, scope: slot.scope, harness: slot.harness,
      revision, view_harness: harness,
    }), `Cleared the ${slotLabel(slot.scope, slot.harness)} override for ${promptId}.`);
    if (cleared) setConfirmClear(null);
  }

  const targetExists = draft && draftRow?.stored.some((slot) => slot.scope === draft.scope && slot.harness === draft.harness);
  const editing = draft !== null && row?.id === draft.promptId;

  return <Modal label="Prompt overrides" title="Prompt overrides" size="xl" onClose={requestClose} dismissable={!busy}
    subtitle="Change the prompts AMF sends for headless AI calls. The nearest scope wins: feature, then project, then global, then the built-in default."
    footer={<>
      <span className="muted small">{view ? `Context: ${view.context_label}` : ""}</span>
      <button className="btn btn-secondary" disabled={busy} onClick={requestClose}>Done</button>
    </>}>
    {fromPrecall && <div className="callout callout-accent" role="note"><p>Opened from a pending AI call. Saved changes apply when you continue that call; its preview was rendered before your edit.</p></div>}
    <div className="library-toolbar">
      <Field label="Override context"><select disabled={busy} value={contextKey(context)} onChange={(event) => {
        const next = JSON.parse(event.target.value) as OverrideContext;
        guard("switch context", () => { setContext(next); setConfirmClear(null); setError(null); setNotice(null); });
      }}>
        {!contexts.some((candidate) => contextKey(candidate.value) === contextKey(context)) &&
          <option value={contextKey(context)}>Context no longer available</option>}
        {contexts.map((candidate) => <option key={contextKey(candidate.value)} value={contextKey(candidate.value)}>{candidate.label}</option>)}
      </select></Field>
      <Field label="Show templates for"><select disabled={busy} value={harness ?? view?.harness ?? ""} onChange={(event) => setHarness(event.target.value as AgentSlug)}>
        {HARNESSES.map((slug) => <option key={slug} value={slug}>{HARNESS_LABELS[slug]}</option>)}
      </select></Field>
      <button className="btn btn-secondary btn-sm" disabled={overrides.isFetching || busy} onClick={() => void overrides.refetch()}>Refresh</button>
    </div>
    {overrides.error && <p role="alert" className="error-text">{asGuiError(overrides.error).message}</p>}
    {view?.project_config_error && <div className="callout callout-warning" role="alert">
      <p><strong>Project overrides are ignored.</strong> {view.project_config_error}</p>
    </div>}
    {pendingDiscard && <div className="callout callout-warning" role="alertdialog" aria-label="Discard unsaved template">
      <p>Discard your unsaved template changes and {pendingDiscard.label}?</p>
      <button className="btn btn-secondary btn-sm" onClick={() => setPendingDiscard(null)}>Keep editing</button>
      <button className="btn btn-warning btn-sm" onClick={() => { const next = pendingDiscard.run; setPendingDiscard(null); setDraft(null); next(); }}>Discard changes</button>
    </div>}
    <div className="library-browser">
      <div className="library-list" role="group" aria-label="Headless prompts">
        {overrides.isLoading && <p className="muted">Loading prompts…</p>}
        {view?.rows.map((candidate) => <button key={candidate.id} disabled={busy}
          ref={row?.id === candidate.id ? activeItem : undefined}
          className={`library-item ${row?.id === candidate.id ? "library-item-active" : ""}`}
          aria-pressed={row?.id === candidate.id}
          onClick={() => candidate.id !== row?.id && guard("open another prompt", () => { setSelectedId(candidate.id); setConfirmClear(null); setError(null); setNotice(null); })}>
          <span className="library-item-heading"><strong>{candidate.title}</strong>
            <span className={`badge ${candidate.source === "built_in" ? "" : "overrides-badge-active"}`}>
              {candidate.source === "built_in" ? "Built-in" : slotLabel(candidate.source, candidate.source_harness)}
            </span>
          </span>
          <span className="muted small mono">{candidate.id}</span>
        </button>)}
      </div>
      <div className="library-detail">
        {!row && !overrides.isLoading && <EmptyState icon="file" title="Select a prompt">Choose a prompt to see its effective template.</EmptyState>}
        {row && <>
          <h3>{row.title}</h3>
          <p className="muted">{row.summary} <code>{row.id}</code></p>
          <p className="small"><strong>Placeholders:</strong>{" "}
            {row.placeholders.length === 0 ? <span className="muted">none</span> :
              row.placeholders.map((name) => <code key={name} className="overrides-token">{`{{${name}}}`}</code>)}
          </p>
          {notice && <p role="status" className="muted">{notice}</p>}
          {error && <p role="alert" className="error-text">{error}</p>}
          {editing && draft ? <section className="overrides-editor" aria-label="Edit override">
            <div className="overrides-editor-targets">
              <Field label="Save to scope"><select disabled={busy} value={draft.scope}
                onChange={(event) => setDraft({ ...draft, scope: event.target.value as OverrideScope })}>
                {view?.scopes.map((scope) => <option key={scope.scope} value={scope.scope} disabled={!scope.available}>
                  {scope.label}{scope.available ? "" : ` (unavailable: ${scope.reason ?? "not here"})`}
                </option>)}
              </select></Field>
              <Field label="Harness"><select disabled={busy} value={draft.harness ?? ""}
                onChange={(event) => setDraft({ ...draft, harness: (event.target.value || null) as AgentSlug | null })}>
                <option value="">Shared (all harnesses)</option>
                {HARNESSES.map((slug) => <option key={slug} value={slug}>{HARNESS_LABELS[slug]} only</option>)}
              </select></Field>
            </div>
            <Field label="Template" hint="Placeholders are not validated: a missing token is sent literally. While an override stands, built-in prompt updates no longer apply.">
              <textarea className="overrides-textarea" disabled={busy} rows={14} value={draft.text}
                onChange={(event) => setDraft({ ...draft, text: event.target.value })} />
            </Field>
            {targetExists && <p className="muted small">This replaces the existing {slotLabel(draft.scope, draft.harness)} override.</p>}
            {stale && <div className="callout callout-warning" role="alert">
              <p>This prompt’s overrides changed outside this window. Reload to use the current version as the base for your draft; your text is kept.</p>
              <button className="btn btn-secondary btn-sm" disabled={busy} onClick={reloadDraftBase}>Reload current version</button>
            </div>}
            {!draft.text.trim() && <p className="muted small">An empty template can’t be saved. Clear the override instead.</p>}
            <div className="overrides-actions">
              <button className="btn btn-ghost" disabled={busy} onClick={() => guard("cancel this edit", () => undefined)}>Cancel edit</button>
              <button className="btn btn-primary" disabled={busy || stale || !draft.text.trim()} onClick={() => void save()}>
                {busy && <Spinner />}Save override
              </button>
            </div>
          </section> : <>
            <h4>Effective template <span className="badge">{slotLabel(row.source, row.source_harness)}</span></h4>
            <pre className="library-preview" aria-label="Effective template">{row.effective_template}</pre>
            {row.source !== "built_in" && <details><summary>Built-in default</summary>
              <pre className="library-preview" aria-label="Built-in default">{row.default_template}</pre></details>}
            <h4>Stored overrides</h4>
            {row.stored.length === 0 && <p className="muted small">None in this context. The prompt uses the built-in default.</p>}
            <ul className="overrides-slots">
              {row.stored.map((slot) => {
                const label = slotLabel(slot.scope, slot.harness);
                const scopeOption = view?.scopes.find((scope) => scope.scope === slot.scope);
                return <li key={label}>
                  <span><strong>{label}</strong>{slot.scope === row.source && (slot.harness ?? null) === (row.source_harness ?? null) && <span className="badge overrides-badge-active">in effect</span>}</span>
                  <span className="overrides-slot-actions">
                    <button className="btn btn-secondary btn-sm" disabled={busy || draft !== null || !scopeOption?.available} onClick={() => startEdit(row, slot)} aria-label={`Edit ${label}`}>Edit</button>
                    <button className="btn btn-ghost btn-sm" disabled={busy || draft !== null || !scopeOption?.available} onClick={() => { setConfirmClear({ promptId: row.id, slot, revision: row.revision }); setError(null); setNotice(null); }} aria-label={`Clear ${label}`}>Clear…</button>
                  </span>
                </li>;
              })}
            </ul>
            {confirmClear && <div className="callout callout-warning" role="alertdialog" aria-label="Confirm clear override">
              <p>Clear the {slotLabel(confirmClear.slot.scope, confirmClear.slot.harness)} override for {row.title}? Only this override is removed; the prompt then uses the next layer that has a template.</p>
              <pre className="library-preview" aria-label="Template to clear">{confirmClear.slot.template}</pre>
              {clearStale && <p role="alert">This prompt’s overrides changed after you chose Clear. Keep it, then review the current template before clearing.</p>}
              <button className="btn btn-secondary btn-sm" disabled={busy} onClick={() => setConfirmClear(null)}>Keep it</button>
              <button className="btn btn-danger btn-sm" disabled={busy || clearStale} onClick={() => void clear(confirmClear)}>{busy && <Spinner />}Clear override</button>
            </div>}
            <button className="btn btn-primary" disabled={busy || draft !== null || availableScopes.length === 0} onClick={() => startEdit(row, null)}>New override…</button>
          </>}
        </>}
      </div>
    </div>
  </Modal>;
}
