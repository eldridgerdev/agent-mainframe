import { useEffect, useState } from "react";
import { asGuiError, type FeatureTarget } from "./api";
import CompanionIntegrationDialog from "./CompanionIntegrationDialog";
import { prDestinationCatalog, type DestinationCatalog, type DestinationFeature, type PrFixTarget } from "./prTriageApi";

/** Companion banner text. `commits_ahead` is null whenever Git could not prove
 * the count, which is shown as such and never as zero. */
export function companionSummary(feature: DestinationFeature): string {
  const ahead = feature.companion?.commits_ahead;
  if (ahead == null) return "commits ahead unavailable";
  const count = ahead === 1 ? "1 commit ahead" : `${ahead} commits ahead`;
  // The count is committed work only; say so when the tree holds more.
  return feature.companion?.uncommitted_changes ? `${count}, committed only; uncommitted changes in its working tree` : count;
}

/** Why a session row cannot be chosen, or null. Mirrors the stale test of the
 * fix-target select: stopped, blocked, or no longer in the live catalog. */
export function rowDisabledReason(feature: DestinationFeature, session: PrFixTarget, selectable?: ReadonlySet<string>): string | null {
  if (feature.blocked) return feature.blocked;
  if (session.unavailable) return session.unavailable;
  if (session.stopped) return "Stopped: start it from its session tab first";
  if (selectable && !selectable.has(session.target.session_id)) return "Refreshing the agent list; try again in a moment";
  return null;
}

/** Pure rows view over a loaded catalog (sessions in this feature and existing
 * companions with their sessions). Choosing a row only reports its key; the
 * caller still needs an explicit action to prepare or send anything. */
export function DestinationRows({ catalog, selectedKey, choiceKey, onChoose, onIntegrate, disabled, selectableKeys }: {
  catalog: DestinationCatalog;
  /** The resolved selection (may be a preselected existing companion session). */
  selectedKey: string | null;
  /** The user's explicit choice: null = untouched, "" = cleared. */
  choiceKey: string | null;
  onChoose: (key: string) => void;
  onIntegrate: (companion: DestinationFeature) => void;
  disabled?: boolean;
  /** Session ids the parent can currently resolve (its own fix-target list). A
   * catalog row outside it is disabled instead of silently doing nothing. */
  selectableKeys?: ReadonlySet<string>;
}) {
  const known = new Set([catalog.opened, ...catalog.companions].flatMap((f) => f.sessions.map((s) => s.target.session_id)));
  const staleChoice = choiceKey !== null && choiceKey !== "" && !known.has(choiceKey);
  const row = (feature: DestinationFeature, session: PrFixTarget) => {
    const reason = rowDisabledReason(feature, session, selectableKeys);
    const key = session.target.session_id;
    const chosen = selectedKey === key;
    return (
      <li key={key} className="destination-session">
        <button type="button" className={`btn btn-ghost btn-sm${chosen ? " is-selected" : ""}`} aria-pressed={chosen}
          disabled={disabled || !!reason} title={reason ?? undefined} onClick={() => onChoose(key)}>
          {chosen ? "Selected" : "Use"}
        </button>
        <span>{session.label}</span>
        <span className="muted small">{session.harness}{session.stopped ? " (stopped)" : ""}</span>
        {reason && <span className="muted small"> {reason}</span>}
      </li>
    );
  };
  const names = [catalog.opened, ...catalog.companions];
  return (
    <section className="destination-catalog" aria-label="Fix destinations">
      {staleChoice && <p className="small" role="status">The previously selected session is no longer available or changed; choose again.</p>}
      <details className="destination-details" open={catalog.companions.length > 0 || undefined}>
        <summary>Destinations ({names.length === 1 ? "this feature" : `this feature and ${catalog.companions.length} triage companion${catalog.companions.length === 1 ? "" : "s"}`})</summary>
        <div className="destination-feature">
          <strong>This feature: {catalog.opened.name}</strong>
          <span className="muted small"> {catalog.opened.branch}{catalog.opened.running ? "" : " (stopped)"}</span>
          {catalog.opened.blocked && <p className="muted small">{catalog.opened.blocked}</p>}
          {catalog.opened.sessions.length === 0 && !catalog.opened.blocked && <p className="muted small">No agent sessions in this feature.</p>}
          <ul>{catalog.opened.sessions.map((s) => row(catalog.opened, s))}</ul>
        </div>
        {catalog.companions.map((feature) => (
          <div key={feature.target.feature_id} className="destination-feature destination-companion" role="group" aria-label={`Triage companion ${feature.name}`}>
            <strong>Triage companion: {feature.name}</strong>
            <span className="muted small"> {feature.branch} · {feature.running ? "running" : "stopped"} · {feature.workdir}</span>
            {feature.blocked && <p className="muted small">{feature.blocked}</p>}
            {feature.companion?.commits_ahead == null && feature.companion?.commits_ahead_error && <p className="muted small">Commits ahead unknown: {feature.companion.commits_ahead_error}</p>}
            {feature.sessions.length === 0 && !feature.blocked && <p className="muted small">No agent sessions in this companion.</p>}
            <ul>{feature.sessions.map((s) => row(feature, s))}</ul>
          </div>
        ))}
      </details>
      {catalog.companions.map((feature) => {
        const why = feature.blocked ?? (feature.companion?.commits_ahead == null ? "Commits ahead could not be established" : null);
        return (
          <p key={feature.target.feature_id} className="destination-integrate small">
            Triage feature {feature.name} ({companionSummary(feature)}) –{" "}
            <button type="button" className="btn btn-ghost btn-sm" disabled={disabled || !!why} title={why ?? undefined} onClick={() => onIntegrate(feature)}>
              Integrate PR #{catalog.pr_number} fixes…
            </button>
            {why && <span className="muted"> {why}</span>}
          </p>
        );
      })}
    </section>
  );
}

/** Loads the catalog for one opened feature + PR and renders it. It only reads:
 * a failed load shows why and lists nothing, so no row is ever invented. */
export default function DestinationPicker({ target, prNumber, headSha, refreshKey, selectedKey, choiceKey, onChoose, disabled, selectableKeys }: {
  target: FeatureTarget;
  prNumber: number | null | undefined;
  /** The PR head being shown; companions are only valid for exactly this commit. */
  headSha: string | null | undefined;
  /** Change to reload (for example after the fix-target list refreshes). */
  refreshKey?: string;
  selectedKey: string | null;
  choiceKey: string | null;
  onChoose: (key: string) => void;
  disabled?: boolean;
  selectableKeys?: ReadonlySet<string>;
}) {
  const [catalog, setCatalog] = useState<DestinationCatalog | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [integrate, setIntegrate] = useState<DestinationFeature | null>(null);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    if (!prNumber || !headSha) { setCatalog(null); return; }
    let live = true;
    prDestinationCatalog({ project_id: target.project_id, feature_id: target.feature_id }, prNumber, headSha)
      .then((next) => { if (live) { setCatalog(next); setError(null); } })
      .catch((e) => { if (live) { setCatalog(null); setError(asGuiError(e).message); } });
    return () => { live = false; };
  }, [target.project_id, target.feature_id, prNumber, headSha, refreshKey, reload]);
  if (!prNumber || !headSha) return null;
  return (
    <>
      {error && <p className="muted small">Destinations unavailable: {error}</p>}
      {catalog && <DestinationRows catalog={catalog} selectedKey={selectedKey} choiceKey={choiceKey} onChoose={onChoose} onIntegrate={setIntegrate} disabled={disabled} selectableKeys={selectableKeys} />}
      {integrate && <CompanionIntegrationDialog target={integrate.target} prNumber={catalog?.pr_number ?? prNumber}
        onClose={() => { setIntegrate(null); setReload((n) => n + 1); }} />}
    </>
  );
}
