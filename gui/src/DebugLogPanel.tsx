import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { asGuiError } from "./api";
import { debugLogLoad, LogLevel } from "./debugLogApi";
import { Modal, Spinner } from "./ui";

export default function DebugLogPanel({ onClose }: { onClose: () => void }) {
  const [level, setLevel] = useState<LogLevel | "">("");
  const [search, setSearch] = useState("");
  const [copied, setCopied] = useState<string | null>(null);
  const [hasLoaded, setHasLoaded] = useState(false);
  const query = useQuery({
    queryKey: ["debug-log"], queryFn: debugLogLoad, retry: false,
    staleTime: 0, refetchOnWindowFocus: false,
  });
  // Cached data is useful only after this opening has successfully loaded it.
  useEffect(() => {
    if (query.isFetchedAfterMount && query.isSuccess) setHasLoaded(true);
  }, [query.isFetchedAfterMount, query.isSuccess]);
  const view = hasLoaded ? query.data : undefined;
  const needle = search.trim().toLocaleLowerCase();
  const entries = view?.entries.filter((entry) => (!level || entry.level === level)
    && (!needle || `${entry.context}\n${entry.message}`.toLocaleLowerCase().includes(needle))) ?? [];

  async function copyVisible() {
    const text = entries.map((entry) => `${entry.timestamp} ${entry.level} ${entry.context}: ${entry.message}`).join("\n");
    try {
      await navigator.clipboard.writeText(text);
      setCopied(`Copied ${entries.length} ${entries.length === 1 ? "entry" : "entries"}. Paste them into a bug report.`);
    } catch {
      setCopied("Could not copy to the clipboard.");
    }
  }

  return <Modal label="Debug log" title="Debug log" size="xl" onClose={onClose}
    subtitle="Recent AMF activity, oldest first. Refresh to load new entries."
    footer={<><span className="muted small modal-footer-hint">Esc to close</span><button className="btn btn-primary" onClick={onClose}>Close</button></>}>
    <div className="row debug-log-toolbar">
      <label>Level <select value={level} onChange={(event) => setLevel(event.target.value as LogLevel | "")}>
        <option value="">All levels</option>
        {["DEBUG", "INFO", "WARN", "ERROR"].map((value) => <option key={value}>{value}</option>)}
      </select></label>
      <label>Filter entries <input type="search" placeholder="Text in the source or message" value={search} onChange={(event) => setSearch(event.target.value)} /></label>
      <button className="btn btn-secondary" disabled={query.isFetching} onClick={() => void query.refetch()}>
        {query.isFetching && <Spinner />}Refresh log
      </button>
      <button className="btn btn-secondary" disabled={entries.length === 0} onClick={() => void copyVisible()}>Copy visible entries</button>
    </div>
    {copied && <p role="status">{copied}</p>}
    {query.isFetching && <p role="status">Loading recent log entries…</p>}
    {query.error && <div className="callout callout-danger" role="alert">
      Could not load debug log: {asGuiError(query.error).message}
      {view && <p>Showing the previous load. Refresh to try again.</p>}
    </div>}
    {view && <>
      <p className="muted">Showing {entries.length} of {view.entries.length} entries in the latest {view.limit}.
        {view.shared_history ? " Includes entries from earlier sessions." : " AMF's database is unavailable, so only this session's entries are shown."}
      </p>
      {entries.length === 0 ? <p>{view.entries.length === 0 ? "No log entries yet." : "No entries match these filters."}</p>
        : <ol className="debug-log-entries" aria-label="Log entries">
          {entries.map((entry, index) => <li key={index} className={`debug-log-entry debug-log-${entry.level.toLowerCase()}`}>
            <div><time dateTime={entry.timestamp}>{new Date(entry.timestamp).toLocaleString()}</time>{" "}
              <strong>{entry.level}</strong>{" "}<span className="mono">{entry.context}</span></div>
            <pre>{entry.message}</pre>
          </li>)}
        </ol>}
    </>}
  </Modal>;
}
