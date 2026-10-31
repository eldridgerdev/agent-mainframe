import { useEffect, useRef, useState } from "react";
import { reviewDestinationCatalog, reviewDestinationPreview, type AgentSlug, type ModeSlug, type ReviewAction, type ReviewDestinationCatalog, type ReviewDestinationChoice, type ReviewDestinationPreview, type ReviewView } from "./api";
import { Field, Modal, Spinner } from "./ui";

/** Preview and prepare a destination. Editing these fields never creates or starts anything. */
export default function ReviewFeedbackDestinationDialog({ view, error, onAct, onClose }: {
  view: ReviewView; error: string | null; onAct: (action: ReviewAction) => Promise<boolean>; onClose: () => void;
}) {
  const [catalog, setCatalog] = useState<ReviewDestinationCatalog | null>(null);
  const [kind, setKind] = useState<"current" | "existing" | "dedicated" | "companion">("current");
  const [selected, setSelected] = useState("");
  const [agent, setAgent] = useState<AgentSlug>("claude");
  const [mode, setMode] = useState<ModeSlug>("supervibe");
  const [branch, setBranch] = useState("");
  const [review, setReview] = useState(false);
  const [chrome, setChrome] = useState(false);
  const [hookChoice, setHookChoice] = useState("");
  const [preview, setPreview] = useState<{ value: ReviewDestinationPreview; choice: ReviewDestinationChoice; key: string; revision: number } | null>(null);
  const [approved, setApproved] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [discard, setDiscard] = useState(false);
  const pending = useRef(false);
  const available = useRef(true);
  const request = useRef(0);
  const closeCurrent = useRef<() => void>(() => {});
  const currentView = useRef(view);
  currentView.current = view;
  async function load(initial = false) {
    if (pending.current) return;
    pending.current = true; setBusy(true); setNotice(null); setPreview(null);
    const id = ++request.current;
    const source = currentView.current;
    try {
      const loaded = await reviewDestinationCatalog(source);
      if (!available.current || id !== request.current || source.revision !== currentView.current.revision) return;
      setCatalog(loaded);
      if (initial) {
        const row = loaded.sessions.find(row => row.current && !row.unavailable);
        setSelected(row ? JSON.stringify(row.target) : "");
        setAgent(loaded.agents.find(row => !row.unavailable)?.agent ?? "claude");
        setBranch(loaded.default_branch);
      }
    } catch (failure) { if (available.current && id === request.current) setNotice(message(failure)); }
    finally { if (available.current && id === request.current) { pending.current = false; setBusy(false); } }
  }
  useEffect(() => {
    available.current = true;
    pending.current = false;
    void load(true);
    return () => { available.current = false; request.current++; pending.current = false; };
  }, []); // Retain edited fields when polling changes the review; replay safely in StrictMode.
  const rows = catalog?.sessions.filter(row => kind === "current" ? row.current : !row.current) ?? [];
  const row = rows.find(row => JSON.stringify(row.target) === selected);
  const choice: ReviewDestinationChoice | null = kind === "current" || kind === "existing"
    ? row && !row.unavailable ? { kind: "existing", target: row.target } : null
    : kind === "dedicated" ? { kind: "dedicated", agent }
      : branch.trim() && catalog?.companion_available ? { kind: "companion", settings: { branch: branch.trim(), agent, mode, review, enable_chrome: chrome }, hook_choice: hookChoice || null } : null;
  const choiceKey = JSON.stringify(choice);
  function edit(change: () => void) {
    if (pending.current) return;
    change(); setDirty(true); setPreview(null); setApproved(false); setNotice(null);
  }
  function close() {
    if (pending.current) return;
    if (dirty) setDiscard(true); else onClose();
  }
  closeCurrent.current = close;
  useEffect(() => {
    const node = document.querySelector<HTMLElement>('[role="dialog"][aria-label="Feedback destination"]');
    if (!node) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    function root() { return node!.querySelector<HTMLElement>('[role="alertdialog"]') ?? node!; }
    function controls() { return Array.from(root().querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])')); }
    function focusInside() { (controls()[0] ?? node!).focus(); }
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault(); event.stopImmediatePropagation(); closeCurrent.current();
      } else if (event.key === "Tab") {
        const items = controls();
        const index = items.indexOf(document.activeElement as HTMLElement);
        if (!items.length || index < 0 || (event.shiftKey ? index === 0 : index === items.length - 1)) {
          event.preventDefault(); (event.shiftKey ? items[items.length - 1] ?? node! : items[0] ?? node!).focus();
        }
      }
    }
    function onFocus(event: FocusEvent) { if (event.target instanceof Node && !root().contains(event.target)) focusInside(); }
    window.addEventListener("keydown", onKey, true);
    document.addEventListener("focusin", onFocus);
    node.focus();
    return () => {
      window.removeEventListener("keydown", onKey, true);
      document.removeEventListener("focusin", onFocus);
      if (previous?.isConnected && !(previous instanceof HTMLButtonElement && previous.disabled)) previous.focus();
      else document.querySelector<HTMLElement>('[role="dialog"][aria-label="Final Review"]')?.focus();
    };
  }, []);
  useEffect(() => {
    const dialog = document.querySelector<HTMLElement>('[role="dialog"][aria-label="Feedback destination"]');
    if (discard) dialog?.querySelector<HTMLElement>('[aria-label="Discard destination choices"] button')?.focus();
    else if (dialog && !dialog.contains(document.activeElement)) dialog.querySelector<HTMLElement>('select:not([disabled])')?.focus();
  }, [discard]);
  async function inspect() {
    if (!choice || pending.current) return;
    pending.current = true; setBusy(true); setNotice(null); setPreview(null); setApproved(false);
    const id = ++request.current;
    const source = currentView.current;
    try {
      const value = await reviewDestinationPreview(source, choice);
      if (available.current && id === request.current && source.revision === currentView.current.revision) setPreview({ value, choice, key: choiceKey, revision: source.revision });
    } catch (failure) { if (available.current && id === request.current) setNotice(message(failure)); }
    finally { if (available.current && id === request.current) { pending.current = false; setBusy(false); } }
  }
  const fresh = preview && preview.key === choiceKey && preview.revision === view.revision;
  async function prepare() {
    if (!fresh || !preview || pending.current || (preview.value.resource_warning && !approved)) return;
    pending.current = true; setBusy(true); setNotice(null);
    const id = ++request.current;
    try {
      if (await onAct({ kind: "prepare_destination", choice: preview.choice, preview_revision: preview.value.revision, approved })) onClose();
      else if (available.current) setNotice("Preparation was refused. Your choices are retained; refresh the preview before retrying.");
    } finally { if (available.current && id === request.current) { pending.current = false; setBusy(false); } }
  }
  return <Modal label="Feedback destination" title="Feedback destination" size="lg" onClose={close} dismissable={!busy}>
    <p>Completion opens an unsent feedback draft. Your existing composer text is preserved. Preparing a new destination creates and starts its agent before completion; cancelling later keeps those resources.</p>
    {busy && <p role="status"><Spinner /> Reading or preparing destination…</p>}
    {(notice || error) && <p role="alert">{notice}{error && ` ${error}`}</p>}
    {!catalog ? <button className="btn btn-secondary" disabled={busy} onClick={() => void load(true)}>Retry loading destinations</button> : <>
      <Field label="Destination type"><select value={kind} disabled={busy} onChange={event => edit(() => { setKind(event.target.value as typeof kind); setSelected(""); })}>
        <option value="current">Current feature session</option><option value="dedicated">Dedicated Final Review session</option><option value="existing">Another feature session</option>
        <option value="companion" disabled={!catalog.companion_available}>New companion worktree</option>
      </select></Field>
      {(kind === "current" || kind === "existing") && <Field label="Agent session"><select value={selected} disabled={busy} onChange={event => edit(() => setSelected(event.target.value))}>
        <option value="">Choose an agent session</option>{rows.map(row => <option key={JSON.stringify(row.target)} value={JSON.stringify(row.target)} disabled={!!row.unavailable}>{row.label}{row.stopped ? " (stopped; draft only)" : ""}{row.unavailable ? ` — ${row.unavailable}` : ""}</option>)}
      </select>{rows.length === 0 && <p>No sessions are available for this destination type.</p>}</Field>}
      {(kind === "dedicated" || kind === "companion") && <Field label="Agent harness"><select value={agent} disabled={busy} onChange={event => edit(() => setAgent(event.target.value as AgentSlug))}>
        {catalog.agents.map(row => <option key={row.agent} value={row.agent} disabled={!!row.unavailable}>{row.label}{row.unavailable ? ` — ${row.unavailable}` : ""}</option>)}
      </select></Field>}
      {kind === "companion" && <>
        <Field label="Companion branch"><input value={branch} disabled={busy} onChange={event => edit(() => setBranch(event.target.value))} /></Field>
        <Field label="Mode"><select value={mode} disabled={busy} onChange={event => edit(() => setMode(event.target.value as ModeSlug))}><option value="supervibe">SuperVibe</option><option value="vibe">Vibe</option><option value="vibeless">Vibeless</option></select></Field>
        <label className="checkbox"><input type="checkbox" checked={review} disabled={busy} onChange={event => edit(() => setReview(event.target.checked))} /> Review workflow</label>
        <label className="checkbox"><input type="checkbox" checked={chrome} disabled={busy} onChange={event => edit(() => setChrome(event.target.checked))} /> Enable Chrome</label>
        {catalog.hook_prompt && <Field label={catalog.hook_prompt.title}><select value={hookChoice} disabled={busy} onChange={event => edit(() => setHookChoice(event.target.value))}><option value="">Choose a worktree setup option</option>{catalog.hook_prompt.options.map(option => <option key={option}>{option}</option>)}</select></Field>}
        <p>Branches from {catalog.source_branch} at {catalog.source_head}. Uncommitted reviewed changes are not copied. Integrate the companion's commits later from the companion feature's “Integrate companion…” dialog.</p>
      </>}
      <button className="btn btn-secondary" disabled={busy} onClick={() => void load()}>Refresh destinations</button>
      <button className="btn btn-secondary" disabled={busy || !choice} onClick={() => void inspect()}>Preview destination</button>
      {preview && <section className="review-note" aria-label="Destination preview">
        <h3>{preview.value.label}</h3><p>Feedback file: {preview.value.source_path}/.claude/final-review-feedback.md</p>
        {preview.value.destination_path && <p>Destination checkout: {preview.value.destination_path}</p>}
        {preview.value.base_sha && <p>Branch basis: {preview.value.base_branch} at {preview.value.base_sha}</p>}
        <p>{preview.value.creates_companion ? "Creates a companion checkout and stopped feature, runs the chosen worktree hook, then starts its agent if the source and destination are still fresh." : preview.value.creates_session ? "Creates and starts a dedicated Final Review session." : "Uses the selected existing session without starting it."}</p>
        {!fresh && <p role="status">The review changed. Preview again before preparing.</p>}
        {preview.value.resource_warning && <><p role="alert">{preview.value.resource_warning}</p><label className="checkbox"><input type="checkbox" disabled={busy} checked={approved} onChange={event => { if (!pending.current) setApproved(event.target.checked); }} /> Proceed despite this resource warning</label></>}
        <button className="btn btn-primary" disabled={busy || !fresh || (!!preview.value.resource_warning && !approved)} onClick={() => void prepare()}>Prepare destination</button>
      </section>}
    </>}
    <button className="btn btn-secondary" disabled={busy} onClick={close}>Cancel destination selection</button>
    {discard && <div role="alertdialog" aria-label="Discard destination choices"><p>Discard these unsaved destination choices? No resources were created by editing them.</p><button className="btn btn-secondary" onClick={() => setDiscard(false)}>Keep editing</button><button className="btn btn-primary" onClick={onClose}>Discard choices</button></div>}
  </Modal>;
}
function message(failure: unknown): string { return failure && typeof failure === "object" && "message" in failure ? String(failure.message) : String(failure); }
