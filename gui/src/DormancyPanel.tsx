import { useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  DormancyStopResult,
  DormancyView,
  DormantFeatureView,
  DormantObservation,
  EditorCleanup,
  FeatureTarget,
  asGuiError,
  dormancyLoad,
  dormancyStop,
} from "./api";
import { EmptyState, Icon, Modal, Spinner } from "./ui";

export const DORMANCY_KEY = ["dormancy"];

/** Coarse age, as the TUI's dormant list shows it: minutes, then hours, then days. */
export function humanize(secs: number): string {
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86_400) return `${Math.floor(secs / 3600)}h`;
  return `${Math.floor(secs / 86_400)}d`;
}

const when = (iso: string) => new Date(iso).toLocaleString(undefined, {
  month: "short", day: "numeric", hour: "2-digit", minute: "2-digit",
});

const key = (observation: { target: FeatureTarget }) =>
  `${observation.target.project_id}:${observation.target.feature_id}`;

/** What stopping does to editors, stated before the user confirms. */
function editorPolicy(view: DormancyView): string {
  return view.kill_editor_on_stop
    ? "Editor windows AMF opened for these features are closed too. A window AMF did not open, one whose process now belongs to something else, or one sharing its VS Code instance with other windows is left running and reported."
    : "Editor cleanup is off (kill_editor_on_stop), so no editor window is examined or closed.";
}

function EditorReport({ editors }: { editors: EditorCleanup | null }) {
  if (!editors) return <p className="muted">Editor cleanup is off; no editor was examined.</p>;
  const lines = [
    ...editors.killed.map((editor) => `Closed ${editor.name} (${editor.processes} process${editor.processes === 1 ? "" : "es"} ended)`),
    ...editors.skipped.map((editor) => editor.deliberate
      ? `Left ${editor.name} running: ${editor.reason}`
      : `${editor.name} had already closed`),
    ...editors.pending.map((name) => `${name} is still opening; AMF closes it once it can identify the window`),
  ];
  if (lines.length === 0) return <p className="muted">No tracked editor.</p>;
  return <ul className="dormancy-editors" aria-label="Editor cleanup">
    {lines.map((line, index) => <li key={index}>{line}</li>)}
  </ul>;
}

function Row({ row, selected, disabled, onToggle, onOpen }: {
  row: DormantFeatureView; selected: boolean; disabled: boolean;
  onToggle: () => void; onOpen: () => void;
}) {
  const { observation } = row;
  return <li className={selected ? "dormancy-row dormancy-row-selected" : "dormancy-row"}>
    <input type="checkbox" checked={selected} disabled={disabled} onChange={onToggle}
      aria-label={`Select ${observation.feature_name}`} />
    <div className="dormancy-row-main">
      <div className="dormancy-row-title">
        <strong>{observation.feature_name}</strong>
        <span className="muted">{row.project_name}</span>
        {row.is_worktree && <span className="chip">Worktree</span>}
        {row.editor_alive && <span className="chip chip-warning">Editor open</span>}
      </div>
      <div className="dormancy-why">
        <span>Idle <strong>{humanize(row.idle_secs)}</strong> · no agent output since {when(observation.last_activity)}</span>
        <span>Unattended <strong>{humanize(row.unattended_secs)}</strong> · last opened {when(observation.last_accessed)}</span>
      </div>
    </div>
    <button className="btn btn-ghost btn-sm" disabled={disabled} onClick={onOpen}
      title="Opening a session counts as attention, so the feature stops being dormant">Open</button>
  </li>;
}

/// Running features that are idle *and* unattended (the TUI's `z`), with an
/// explicitly confirmed stop. The backend re-checks every selected feature
/// before stopping it, so this list is a proposal, never the authority.
export default function DormancyPanel({ onClose, onOpenFeature }: {
  onClose: () => void;
  onOpenFeature: (target: FeatureTarget) => void;
}) {
  const query = useQuery({
    queryKey: DORMANCY_KEY,
    queryFn: dormancyLoad,
    retry: false,
    staleTime: 0,
    refetchOnWindowFocus: false,
  });
  const [selected, setSelected] = useState<Record<string, DormantObservation>>({});
  const [confirming, setConfirming] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [results, setResults] = useState<DormancyStopResult[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const inFlight = useRef(false);
  const view = query.data;
  const chosen = Object.values(selected);

  function toggle(row: DormantFeatureView) {
    setSelected((current) => {
      const next = { ...current };
      if (next[key(row.observation)]) delete next[key(row.observation)];
      else next[key(row.observation)] = row.observation;
      return next;
    });
  }

  async function refresh() {
    setError(null);
    const fresh = await query.refetch();
    if (!fresh.data) return;
    // A refreshed row is a fresh observation; a row that left the list leaves
    // the selection, and the user is told rather than left guessing.
    const listed = new Map(fresh.data.features.map((row) => [key(row.observation), row.observation]));
    const kept: Record<string, DormantObservation> = {};
    let dropped = 0;
    for (const id of Object.keys(selected)) {
      const observation = listed.get(id);
      if (observation) kept[id] = observation; else dropped++;
    }
    setSelected(kept);
    setNotice(dropped > 0 ? `${dropped} selected feature(s) are no longer dormant and were deselected.` : null);
  }

  async function stop() {
    if (inFlight.current || chosen.length === 0) return;
    inFlight.current = true;
    setStopping(true);
    setError(null);
    try {
      const outcome = await dormancyStop(chosen);
      setResults(outcome);
      setSelected({});
      setConfirming(false);
      void query.refetch();
    } catch (err) {
      setError(asGuiError(err).message);
    } finally {
      inFlight.current = false;
      setStopping(false);
    }
  }

  const footer = results
    ? <><button className="btn btn-secondary" onClick={() => { setResults(null); setNotice(null); }}>Back to dormant features</button>
      <button className="btn btn-primary" onClick={onClose}>Done</button></>
    : confirming
      ? <><button className="btn btn-ghost" disabled={stopping} onClick={() => setConfirming(false)}>Back</button>
        <button className="btn btn-danger" disabled={stopping} onClick={() => void stop()}>
          {stopping && <Spinner />}Stop {chosen.length} feature{chosen.length === 1 ? "" : "s"}
        </button></>
      : <><button className="btn btn-ghost" onClick={onClose}>Close</button>
        {view?.enabled && view.features.length > 0 && <button className="btn btn-warning" disabled={chosen.length === 0}
          onClick={() => { setNotice(null); setConfirming(true); }}>
          Stop selected ({chosen.length})
        </button>}</>;

  return <Modal label="Dormant features" title="Dormant features" size="lg" onClose={onClose}
    dismissable={!stopping}
    subtitle={view?.enabled
      ? `Running, idle over ${view.idle_minutes}m and not opened for over ${view.unattended_hours}h: nobody is watching these and nothing is happening in them.`
      : undefined}
    headerActions={!results && !confirming && <button className="btn btn-ghost btn-sm" disabled={query.isFetching || stopping}
      onClick={() => void refresh()}>{query.isFetching && <Spinner />}Refresh</button>}
    footer={footer}>
    {error && <div className="callout callout-danger" role="alert"><p>{error}</p></div>}
    {query.isLoading && <p className="muted"><Spinner /> Checking tmux activity…</p>}
    {query.error && <div className="callout callout-danger" role="alert">
      <p>Could not check dormancy: {asGuiError(query.error).message}</p>
    </div>}

    {results ? <section aria-label="Stop results" className="dormancy-results">
      <p>Each feature was checked again just before stopping. Only AMF's own tmux session and editor windows were touched.</p>
      <ul>
        {results.map((result) => <li key={key(result)} className={`dormancy-result dormancy-result-${result.outcome}`}>
          <Icon name={result.outcome === "stopped" ? "check" : "alert"} />
          <div>
            <strong>{result.feature_name}</strong>{" "}
            {result.outcome === "stopped" && <><span>stopped</span><EditorReport editors={result.editors} /></>}
            {result.outcome === "refused" && <span>not stopped: {result.message}</span>}
            {result.outcome === "failed" && <span>could not be stopped: {result.message}</span>}
          </div>
        </li>)}
      </ul>
    </section> : confirming && view ? <section role="alertdialog" aria-label="Confirm stopping dormant features" className="dormancy-confirm">
      <p>Stop {chosen.length} dormant feature{chosen.length === 1 ? "" : "s"}? Each feature's tmux session and every session in it ends; saved conversations stay resumable and nothing is deleted.</p>
      <ul>{chosen.map((observation) => <li key={key(observation)}><strong>{observation.feature_name}</strong>{" "}
        <span className="muted">({observation.tmux_session})</span></li>)}</ul>
      <p>{editorPolicy(view)}</p>
      <p className="muted">Each one is checked again first. A feature that was opened, produced output, was stopped or deleted, or otherwise stopped being dormant since this list loaded is skipped and reported.</p>
    </section> : view && (!view.enabled
      ? <EmptyState icon="alert" title="Dormancy detection is off">
          Set dormant_idle_minutes and dormant_last_accessed_hours above 0 in AMF's config to list idle, unattended features.
        </EmptyState>
      : view.features.length === 0
        ? <EmptyState icon="check" title="Nothing is dormant right now">
            No running feature has been idle over {view.idle_minutes}m and unopened over {view.unattended_hours}h. Checked {when(view.checked_at)}.
          </EmptyState>
        : <>
          {notice && <p role="status" className="callout callout-warning">{notice}</p>}
          <p className="muted">Longest idle first. Checked {when(view.checked_at)}. Select features to stop them; nothing is stopped until you confirm.</p>
          <ul className="dormancy-list" aria-label="Dormant features">
            {view.features.map((row) => <Row key={key(row.observation)} row={row}
              selected={!!selected[key(row.observation)]} disabled={stopping}
              onToggle={() => toggle(row)} onOpen={() => onOpenFeature(row.observation.target)} />)}
          </ul>
        </>)}
  </Modal>;
}
