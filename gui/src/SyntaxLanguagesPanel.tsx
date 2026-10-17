import { useEffect, useRef, useState } from "react";
import { asGuiError } from "./api";
import { Modal, Spinner } from "./ui";
import {
  SyntaxLanguageAction, SyntaxLanguageRow, SyntaxLanguagesView, SyntaxInstallView,
  syntaxLanguages, syntaxLanguagesApply, syntaxInstallStatus, syntaxLanguagesChanged, syntaxOperationStarted, SYNTAX_OPERATION_STARTED,
} from "./syntaxApi";

const title = (action: SyntaxLanguageAction) => action === "uninstall" ? "Uninstall" : action === "repair" ? "Repair" : "Install";
const stateName = { available: "Not installed", installed: "Installed", broken: "Needs repair" };

export default function SyntaxLanguagesPanel({ onClose }: { onClose: () => void }) {
  const [view, setView] = useState<SyntaxLanguagesView | null>(null);
  const [operation, setOperation] = useState<SyntaxInstallView | null>(null);
  const [confirmation, setConfirmation] = useState<{ row: SyntaxLanguageRow; action: SyntaxLanguageAction } | null>(null);
  const [loading, setLoading] = useState(false);
  const [requesting, setRequesting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const active = useRef(false), loadGeneration = useRef(0), inFlight = useRef(false);
  const latestOperation = useRef<SyntaxInstallView | null>(null);

  function adopt(next: SyntaxInstallView) {
    const current = latestOperation.current;
    if (current && ((next.operation_id ?? 0) < (current.operation_id ?? 0) || next.completed < current.completed)) return false;
    latestOperation.current = next; setOperation(next); return true;
  }
  async function load(notify = false) {
    if (inFlight.current || latestOperation.current?.running) return;
    const generation = ++loadGeneration.current;
    setLoading(true); setError(null);
    try {
      const fresh = await syntaxLanguages();
      if (!active.current || generation !== loadGeneration.current || !adopt(fresh.operation)) return;
      setView(fresh); setConfirmation(null);
      if (notify && !fresh.operation.running) syntaxLanguagesChanged();
    } catch (err) { if (active.current && generation === loadGeneration.current) setError(asGuiError(err).message); }
    finally { if (active.current && generation === loadGeneration.current) setLoading(false); }
  }
  useEffect(() => {
    active.current = true; void load();
    const started = (event: Event) => {
      if (adopt((event as CustomEvent<SyntaxInstallView>).detail)) {
        loadGeneration.current++; setLoading(false); setConfirmation(null);
      }
    };
    window.addEventListener(SYNTAX_OPERATION_STARTED, started);
    return () => { active.current = false; loadGeneration.current++; window.removeEventListener(SYNTAX_OPERATION_STARTED, started); };
  }, []);

  useEffect(() => {
    if (!operation?.running) return;
    let cancelled = false, pending = false;
    const watched = operation.operation_id ?? 0;
    const timer = window.setInterval(() => {
      if (pending) return;
      pending = true;
      void syntaxInstallStatus().then(next => {
        if (cancelled || !active.current || (next.operation_id ?? 0) < watched || !adopt(next)) return;
        if (!next.running) { void load(); syntaxLanguagesChanged(next); }
      }).catch(err => {
        if (!cancelled && active.current) setError(`Could not read parser progress: ${asGuiError(err).message}`);
      }).finally(() => { pending = false; });
    }, 500);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [operation?.operation_id, operation?.running]);

  async function apply() {
    if (!confirmation || inFlight.current || latestOperation.current?.running || loading) return;
    inFlight.current = true; setRequesting(true); setError(null);
    loadGeneration.current++; // An older list response may not replace this action.
    try {
      const next = await syntaxLanguagesApply(confirmation.row.key, confirmation.action, confirmation.row.revision);
      if (!active.current) return;
      if (adopt(next)) { setConfirmation(null); syntaxOperationStarted(next); }
    } catch (err) { if (active.current) setError(asGuiError(err).message); }
    finally { inFlight.current = false; if (active.current) setRequesting(false); }
  }
  function close() { if (!inFlight.current) { if (confirmation) setConfirmation(null); else onClose(); } }
  const busy = loading || requesting || !!operation?.running;
  return <Modal label="Syntax languages" title="Syntax languages" size="lg" onClose={close} dismissable={!requesting}
    subtitle="Parsers shared by the desktop and TUI. Your open views and drafts stay in place."
    footer={<><button type="button" className="btn btn-secondary" disabled={busy} onClick={() => void load(true)}>Refresh languages</button>
      <button type="button" className="btn btn-ghost" disabled={requesting} onClick={close}>Close manager</button></>}>
    <p className="muted small">Install or repair clones a grammar from GitHub and compiles it with your C compiler. Uninstall removes its parser and managed source checkout. Files in your project stay unchanged.</p>
    {loading && <p role="status"><Spinner /> Reading parser state…</p>}
    {error && <p role="alert" className="callout callout-danger">{error}</p>}
    {operation?.running && <div role="status" className="callout callout-info"><Spinner />
      {operation.action === "uninstall" ? "Removing" : operation.action === "repair" ? "Repairing" : "Installing"} {operation.language} parser…
      {operation.output && <pre className="syntax-install-output">{operation.output}</pre>}
      <p className="small">This operation continues if you close the manager. Reopen it to see progress.</p>
    </div>}
    {!operation?.running && operation?.message && <p role="status" className="callout callout-info">Last operation: {operation.message}</p>}
    {!operation?.running && operation?.error && <p role="alert" className="callout callout-danger">{operation.error} Parser files may have changed; refresh before retrying.</p>}
    {confirmation && <div role="alertdialog" aria-label={`${title(confirmation.action)} ${confirmation.row.title} parser`} className="callout callout-warning">
      <p>{title(confirmation.action)} the {confirmation.row.title} parser?</p>
      <p>{confirmation.action === "uninstall" ? "Remove its shared parser library and managed source checkout. This language will use plain text until installed again."
        : "Clone its grammar from GitHub and compile a shared parser. Repair replaces the existing managed parser files."}</p>
      <button type="button" className="btn btn-secondary" disabled={requesting} onClick={() => setConfirmation(null)}>Cancel</button>
      <button type="button" className={confirmation.action === "uninstall" ? "btn btn-danger" : "btn btn-primary"} disabled={busy} onClick={() => void apply()}>
        {requesting && <Spinner />}{title(confirmation.action)} parser</button>
    </div>}
    <table className="syntax-languages"><thead><tr><th>Language</th><th>Parser</th><th>Actions</th></tr></thead><tbody>
      {(view?.languages ?? []).map(row => <tr key={row.key}><th scope="row">{row.title}</th><td>{stateName[row.state]}</td><td>
        {row.state === "available" ? <button type="button" className="btn btn-secondary btn-sm" disabled={busy} aria-label={`Install ${row.title} parser`}
          onClick={() => { setConfirmation({ row, action: "install" }); setError(null); }}>Install…</button> : <>
          <button type="button" className="btn btn-secondary btn-sm" disabled={busy} aria-label={`Repair ${row.title} parser`}
            onClick={() => { setConfirmation({ row, action: "repair" }); setError(null); }}>Repair…</button>
          <button type="button" className="btn btn-ghost btn-sm" disabled={busy} aria-label={`Uninstall ${row.title} parser`}
            onClick={() => { setConfirmation({ row, action: "uninstall" }); setError(null); }}>Uninstall…</button>
        </>}
      </td></tr>)}
    </tbody></table>
  </Modal>;
}
