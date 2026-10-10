import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { asGuiError, SessionTarget, Project } from "./api";
import { Modal } from "./ui";

export interface BookmarkRow { slot: number; target: SessionTarget; label: string; stale: boolean }
export default function BookmarksPanel({ projects, onOpen, onClose }: {
  projects: Project[]; onOpen: (target: SessionTarget) => void; onClose: () => void;
}) {
  const query = useQuery({ queryKey: ["bookmarks"], queryFn: () => invoke<BookmarkRow[]>("bookmarks_load"),
    retry: false, staleTime: 0, refetchInterval: 3000 });
  const [loaded, setLoaded] = useState(false);
  useEffect(() => { if (query.isFetchedAfterMount && query.isSuccess) setLoaded(true); }, [query.isFetchedAfterMount, query.isSuccess]);
  const [selected, setSelected] = useState("");
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const [error, setError] = useState("");
  const sessions = projects.flatMap(project => project.features.flatMap(feature => feature.sessions.map(session => ({
    key: JSON.stringify([project.id, feature.id, session.id]),
    target: { project_id: project.id, feature_id: feature.id, session_id: session.id },
    label: `${project.name} / ${feature.name} / ${session.label}`,
  }))));
  async function act(command: string, target: SessionTarget) {
    if (inFlight.current) return;
    inFlight.current = true; setBusy(true); setError("");
    try {
      if (command === "bookmarks_resolve") {
        const fresh = await invoke<SessionTarget>(command, { target });
        onOpen(fresh); onClose();
      } else {
        await invoke(command, { target });
      }
    } catch (cause) { setError(asGuiError(cause).message); }
    finally { await query.refetch(); inFlight.current = false; setBusy(false); }
  }
  const rows = loaded ? query.data : undefined;
  return <Modal label="Session bookmarks" title="Session bookmarks" onClose={() => { if (!inFlight.current) onClose(); }}
    subtitle="Nine shared slots. Adding a tenth evicts the oldest bookmark. Opening a bookmark never starts a stopped session."
    footer={<button className="btn btn-secondary" disabled={busy} onClick={() => { if (!inFlight.current) onClose(); }}>Close</button>}>
    {(error || query.error) && <p role="alert">{error || asGuiError(query.error).message}{query.error && loaded && " Showing the previous load; refresh to retry."}</p>}
    <button className="btn btn-secondary" disabled={busy || query.isFetching} onClick={() => void query.refetch()}>Refresh bookmarks</button>
    {rows && <ol aria-label="Bookmarks">{rows.map(row => <li key={JSON.stringify(row.target)} className="row">
      <button className="btn btn-secondary" disabled={busy || query.isError} onClick={() => void act("bookmarks_resolve", row.target)}>{row.slot}. {row.label}</button>
      <button className="btn btn-ghost" disabled={busy || query.isError} aria-label={`Remove bookmark ${row.slot}`} onClick={() => void act("bookmarks_remove", row.target)}>Remove</button>
    </li>)}</ol>}
    {rows?.length === 0 && <p>No bookmarked sessions yet.</p>}
    {!rows && !query.error && <p role="status">Loading bookmarks…</p>}
    <label>Session to bookmark <select value={selected} disabled={busy} onChange={event => setSelected(event.target.value)}>
      <option value="">Choose a session</option>{sessions.map(session => <option key={session.key} value={session.key}>{session.label}</option>)}
    </select></label>
    <button className="btn btn-primary" disabled={busy || !selected || !rows || query.isError} onClick={() => {
      const session = sessions.find(session => session.key === selected);
      if (session) void act("bookmarks_add", session.target);
    }}>Bookmark session</button>
  </Modal>;
}
