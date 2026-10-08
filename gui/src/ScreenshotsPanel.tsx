import { useCallback, useEffect, useRef, useState } from "react";
import { asGuiError, FeatureTarget } from "./api";
import { EvidenceListing, EvidenceOwner, screenshotCleanup, screenshotCleanupScopes, screenshotImage, screenshotsChanged, screenshotsList } from "./screenshotsApi";
import { Modal, Spinner } from "./ui";
import ScreenshotViewer, { ScreenshotImage } from "./ScreenshotViewer";

export default function ScreenshotsPanel({ target = null, sessionId = null, onClose }: { target?: FeatureTarget | null; sessionId?: string | null; onClose: () => void }) {
  const [listing, setListing] = useState<EvidenceListing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<string | null>(sessionId);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [cleanup, setCleanup] = useState<[EvidenceOwner, boolean][] | null>(null);
  const [confirmScope, setConfirmScope] = useState<string | null>(null);
  const [busyScope, setBusyScope] = useState<string | null>(null);
  const pending = useRef(false);
  const mounted = useRef(true);
  const trigger = useRef<string | null>(null);
  const originalFocus = useRef<HTMLElement | null>(null);
  const generation = useRef(0);
  const refresh = useCallback(async () => {
    if (pending.current) return;
    pending.current = true; const current = generation.current;
    try {
      const next = await screenshotsList({ feature: target, session_id: filter });
      if (mounted.current && current === generation.current) { setListing(next); setError(null); }
    } catch (err) { if (mounted.current && current === generation.current) { setError(asGuiError(err).message); setListing(null); setSelectedKey(null); } }
    finally { if (current === generation.current) pending.current = false; }
  }, [target?.project_id, target?.feature_id, filter]);
  useEffect(() => {
    mounted.current = true; originalFocus.current = document.activeElement as HTMLElement;
    return () => { mounted.current = false; generation.current++; originalFocus.current?.focus(); };
  }, []);
  useEffect(() => {
    generation.current++; setListing(null); setSelectedKey(null); setPage(0);
    let live = true; let ticks = 0;
    // Each selection gets a fresh read even if the previous selection is pending.
    pending.current = false; void refresh();
    const timer = window.setInterval(() => {
      ticks++;
      void screenshotsChanged().then((changed) => { if (live && (changed || ticks % 10 === 0 || !pending.current && ticks === 1)) void refresh(); })
        .catch(() => { if (live && ticks % 10 === 0) void refresh(); });
    }, 1000);
    return () => { live = false; generation.current++; window.clearInterval(timer); };
  }, [refresh]);
  const items = listing?.items ?? [];
  const selected = items.find((item) => item.key === selectedKey) ?? null;
  const selectedIndex = selected ? items.indexOf(selected) : -1;
  const closeImage = useCallback(() => { setSelectedKey(null); window.requestAnimationFrame(() => Array.from(document.querySelectorAll<HTMLButtonElement>("button[data-screenshot-key]")).find((button) => button.dataset.screenshotKey === trigger.current)?.focus()); }, []);
  useEffect(() => {
    if (selectedKey !== null && listing && !selected) closeImage();
  }, [selectedKey, listing, selected, closeImage]);
  const move = (offset: number) => { const item = items[selectedIndex + offset]; if (item) { setSelectedKey(item.key); } };
  async function openCleanup() {
    try { const scopes = await screenshotCleanupScopes(); if (mounted.current) setCleanup(scopes.filter(([o]) => !target || o.project_id === target.project_id && o.feature_id === target.feature_id)); }
    catch (err) { if (mounted.current) setError(asGuiError(err).message); }
  }
  async function clean(scope: string) {
    setBusyScope(scope); generation.current++; pending.current = false; setSelectedKey(null); setListing(null);
    try { await screenshotCleanup(scope); setConfirmScope(null); await openCleanup(); await refresh(); }
    catch (err) { if (mounted.current) setError(asGuiError(err).message); }
    finally { if (mounted.current) setBusyScope(null); }
  }
  const sessions = [...new Map((listing?.owners ?? []).map((owner) => [owner.session_id, owner.session_label])).entries()];
  return <Modal label="Screenshots" title={target ? "Validation screenshots" : "Retained validation screenshots"} size="xl"
    onClose={selected ? closeImage : onClose} subtitle="Evidence captured only for explicit visual-validation requests.">
    {error && <div role="alert" className="callout callout-danger">{error}<button className="btn btn-secondary" onClick={() => void refresh()}>Retry</button></div>}
    {selected ? <ScreenshotViewer identity={`${selected.key}:${selected.sha256}`} caption={selected.caption || selected.image_id}
      provenance={[`${selected.owner.feature_name} · ${selected.owner.session_label} · ${selected.owner.session_id} · ${selected.captured_at}`]}
      index={selectedIndex} total={items.length} load={() => screenshotImage(selected, false)} onMove={move} onClose={closeImage} /> : <>
      <div className="row screenshot-toolbar">
        <button className="btn btn-secondary" onClick={() => void refresh()}>Refresh screenshots</button>
        {target && <label>Producing session <select value={filter ?? ""} onChange={(e) => setFilter(e.target.value || null)}>
          <option value="">All sessions, including deleted sessions</option>
          {filter && !sessions.some(([id]) => id === filter) && <option value={filter}>Selected session</option>}
          {sessions.map(([id, label]) => <option key={id} value={id}>{label} · {id}</option>)}
        </select></label>}
        <button className="btn btn-secondary" onClick={() => void openCleanup()}>Screenshot cleanup…</button>
      </div>
      {!listing && !error && <p role="status"><Spinner /> Discovering evidence…</p>}
      {listing && items.length === 0 && <p>No completed screenshots. New Claude/Codex launches receive their evidence destination. Capture requires an explicit request for visual validation.</p>}
      {listing?.truncated && <p role="status">The evidence processing limit was reached. Clean up older scopes to view more.</p>}
      <div className="screenshot-grid">
        {items.slice(page * 20, (page + 1) * 20).map((item) => <article className="screenshot-card" key={item.key}>
          <button className="screenshot-open" data-screenshot-key={item.key} aria-label={`View screenshot ${item.caption || item.image_id}`} onClick={() => { trigger.current = item.key; setSelectedKey(item.key); }}>
            <ScreenshotImage identity={`${item.key}:${item.sha256}`} caption={item.caption || item.image_id} load={() => screenshotImage(item, true)} /><strong>{item.caption || item.image_id}</strong>
          </button>
          <p className="small muted">{item.owner.feature_name} · {item.owner.session_label}</p><p className="small mono">{item.owner.session_id}</p>
        </article>)}
      </div>
      {items.length > 20 && <div className="row"><button disabled={page === 0} onClick={() => setPage((p) => p - 1)}>Previous page</button><span>Page {page + 1}</span><button disabled={(page + 1) * 20 >= items.length} onClick={() => setPage((p) => p + 1)}>Next page</button></div>}
      {!!listing?.issues.length && <details><summary>Incomplete or unsupported evidence ({listing.issues.length})</summary>{listing.issues.map((issue, i) => <p key={i}>{issue.file}: {issue.message}</p>)}</details>}
      {cleanup && <section aria-label="Screenshot cleanup" className="callout">
        <p>Cleanup removes only the selected producing scope. Retained root and non-git evidence stays until you choose cleanup. Restart its agent session to receive a new destination afterward.</p>
        <button className="btn btn-ghost" onClick={() => { setCleanup(null); setConfirmScope(null); }}>Close cleanup</button>
        {!cleanup.length && <p>No screenshot scopes to clean up.</p>}
        {cleanup.map(([owner, retired]) => <div key={owner.scope_id} className="screenshot-cleanup-row">
          <span>{owner.project_name} / {owner.feature_name} · {owner.session_label} · {owner.scope_id}{retired ? " (retired; removal can be retried)" : ""}</span>
          <button className="btn btn-warning" disabled={busyScope !== null} onClick={() => setConfirmScope(owner.scope_id)}>Clean up this scope…</button>
          {confirmScope === owner.scope_id && <div role="alertdialog" aria-label="Confirm screenshot cleanup"><p>Delete this scope’s screenshot files at {owner.workdir}?</p><button disabled={busyScope !== null} onClick={() => void clean(owner.scope_id)}>{busyScope === owner.scope_id ? "Cleaning…" : "Delete screenshots"}</button><button onClick={() => setConfirmScope(null)}>Cancel</button></div>}
        </div>)}
      </section>}
    </>}
  </Modal>;
}
