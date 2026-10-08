import { useCallback, useEffect, useRef, useState } from "react";
import { asGuiError } from "./api";
import ScreenshotViewer, { ScreenshotImage } from "./ScreenshotViewer";
import { closeRemoteScreenshots, openScreenshotBrowser, remoteScreenshotImage, remoteScreenshots, RemoteListing } from "./screenshotsApi";
import { Modal, Spinner } from "./ui";

export default function PrScreenshotsPanel({ workflowId, prNumber, headSha, onClose }: { workflowId: string; prNumber: number; headSha: string; onClose: () => void }) {
  const [listing, setListing] = useState<RemoteListing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [run, setRun] = useState<number | null>(null);
  const [runPage, setRunPage] = useState(1);
  const [page, setPage] = useState(0);
  const [selected, setSelected] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const epoch = useRef(0);
  const request = useRef<string | null>(null);
  const trigger = useRef<string | null>(null);
  const focus = useRef<HTMLElement | null>(null);
  useEffect(() => { focus.current = document.activeElement as HTMLElement; return () => { focus.current?.focus(); }; }, []);
  useEffect(() => {
    const generation = ++epoch.current; setBusy(true); setListing(null); setSelected(null); setPage(0); setError(null);
    if (request.current) void closeRemoteScreenshots(request.current).catch(() => {});
    const requestId = crypto.randomUUID(); request.current = requestId;
    remoteScreenshots(workflowId, run, runPage, requestId).then((next) => {
      if (generation !== epoch.current) { void closeRemoteScreenshots(next.request_id).catch(() => {}); return; }
      request.current = next.request_id; setListing(next);
    }).catch((err) => { if (generation === epoch.current) setError(asGuiError(err).message); })
      .finally(() => { if (generation === epoch.current) setBusy(false); });
    return () => { epoch.current++; if (request.current) void closeRemoteScreenshots(request.current).catch(() => {}); };
  }, [workflowId, prNumber, headSha, run, runPage, retry]);
  const items = listing?.items ?? [];
  const current = items.find((i) => i.key === selected) ?? null;
  const index = current ? items.indexOf(current) : -1;
  const closeImage = useCallback(() => { setSelected(null); window.requestAnimationFrame(() => Array.from(document.querySelectorAll<HTMLButtonElement>("button[data-screenshot-key]")).find((button) => button.dataset.screenshotKey === trigger.current)?.focus()); }, []);
  const move = (offset: number) => { const item = items[index + offset]; if (item) setSelected(item.key); };
  return <Modal label="PR screenshots" title={`PR #${prNumber} screenshots`} size="xl" onClose={current ? closeImage : onClose}
    subtitle={`Discovered at ${headSha.slice(0, 12)}. Repository files and Actions evidence show their resolved revisions.`}>
    {error && <div role="alert" className="callout callout-danger">{error}<button onClick={() => setRetry((v) => v + 1)}>Retry sources</button></div>}
    {current && listing ? <ScreenshotViewer identity={`${listing.request_id}:${current.key}`} caption={current.caption} provenance={current.provenance}
      index={index} total={items.length} load={() => remoteScreenshotImage(listing.request_id, current.key, false)} onMove={move} onClose={closeImage} /> : <>
      <div className="row screenshot-toolbar">
        <button className="btn btn-secondary" disabled={busy} onClick={() => setRetry((v) => v + 1)}>Refresh sources</button>
        <label>Actions run <select disabled={busy} value={run ?? ""} onChange={(e) => setRun(e.target.value ? Number(e.target.value) : null)}>
          <option value="">Latest completed run at current PR commit with screenshots</option>
          {listing?.runs.map((r) => <option key={r.id} value={r.id}>{r.name} · #{r.id} · current attempt {r.attempt} · {r.head_sha.slice(0, 7)} · {r.status} {r.conclusion}</option>)}
        </select></label>
        <button disabled={busy || runPage <= 1} onClick={() => { setRun(null); setRunPage((p) => p - 1); }}>Newer runs page</button>
        <button disabled={busy || !listing?.more_runs} onClick={() => { setRun(null); setRunPage((p) => p + 1); }}>Older runs page</button>
      </div>
      {busy && <p role="status"><Spinner /> Reading PR images and Actions artifacts…</p>}
      {listing && !listing.selected_run && !run && <p>No completed Actions run with supported unexpired screenshots at the current PR commit.</p>}
      {listing && !items.length && <p>No supported screenshots were discovered.</p>}
      <div className="screenshot-grid">{listing && items.slice(page * 20, (page + 1) * 20).map((item) => <article key={item.key} className="screenshot-card">
        <button className="screenshot-open" data-screenshot-key={item.key} aria-label={`View screenshot ${item.caption}`} onClick={() => { trigger.current = item.key; setSelected(item.key); }}>
          <ScreenshotImage identity={`${listing.request_id}:${item.key}`} caption={item.caption} load={() => remoteScreenshotImage(listing.request_id, item.key, true)} /><strong>{item.caption}</strong>
        </button>{item.provenance.map((label) => <p className="small muted" key={label}>{label}</p>)}
      </article>)}</div>
      {items.length > 20 && <div className="row"><button disabled={page <= 0} onClick={() => setPage((p) => p - 1)}>Previous page</button><span>Page {page + 1}</span><button disabled={(page + 1) * 20 >= items.length} onClick={() => setPage((p) => p + 1)}>Next page</button></div>}
      {!!listing?.galleries.length && <section aria-label="Linked galleries">{listing.galleries.map((gallery, i) => <div className="callout" key={`${gallery.url}:${i}`}>
        <p>{gallery.provenance}</p><p>{gallery.reason}</p><button className="btn btn-secondary" onClick={() => { void openScreenshotBrowser(gallery.url).catch((err) => setError(asGuiError(err).message)); }}>Open gallery in browser</button>
      </div>)}</section>}
      {!!listing?.issues.length && <details open><summary>Source notices ({listing.issues.length})</summary>{listing.issues.map((issue, i) => <p key={i}>{issue.source}: {issue.message}</p>)}</details>}
    </>}
  </Modal>;
}
