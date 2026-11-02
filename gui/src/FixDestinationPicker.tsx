import type { PrFixTarget } from "./prTriageApi";

/** Read-only catalog picker shared by every PR fix handoff (batch fix, single
 * comment fix, AI finding fix). It lists agent sessions in the opened feature
 * and in verified triage companions of the PR, and never chooses for the user:
 * `preselectedTarget` only seeds the selection with an existing companion
 * session, and the caller still needs an explicit click to prepare or send. */

/** Session ids are generated UUIDs, unique across features. */
export const targetKey = (t: PrFixTarget) => t.target.session_id;

/** The existing companion session (running) offered up front, or null. */
export function preselectedTarget(targets: PrFixTarget[]): PrFixTarget | null {
  return targets.find((t) => t.destination_kind === "companion" && !t.stopped) ?? null;
}

/** Resolve a user choice (`null` = untouched, "" = explicitly cleared) against
 * the live catalog. A choice no longer present resolves to nothing: there is no
 * fallback to the first row. */
export function resolveTarget(targets: PrFixTarget[], choice: string | null): PrFixTarget | null {
  if (choice === null) return preselectedTarget(targets);
  return targets.find((t) => targetKey(t) === choice) ?? null;
}

export default function FixDestinationPicker({ targets, choice, onChoose, label, disabled, stoppedMode = "disable", stale: staleProp, placeholder = "Choose an agent session…" }: {
  targets: PrFixTarget[];
  choice: string | null;
  onChoose: (key: string) => void;
  label: string;
  disabled?: boolean;
  stoppedMode?: "disable" | "hide";
  /** Overrides the default staleness test (e.g. same session, new revision). */
  stale?: boolean;
  placeholder?: string;
}) {
  const visible = stoppedMode === "hide" ? targets.filter((t) => !t.stopped) : targets;
  const selected = resolveTarget(targets, choice);
  const stale = staleProp ?? (choice !== null && choice !== "" && !selected);
  const group = (kind: PrFixTarget["destination_kind"]) => visible.filter((t) => t.destination_kind === kind);
  const option = (t: PrFixTarget) => (
    <option key={targetKey(t)} value={targetKey(t)} disabled={t.stopped}>{t.label}{t.stopped ? " (stopped)" : ""}</option>
  );
  const opened = group("opened_feature");
  const companions = group("companion");
  const companionNames = [...new Set(companions.map((t) => t.feature_name))];
  return (
    <label className="fix-destination-picker">
      <span>{label}</span>
      <select aria-label={label} value={stale ? "__stale" : selected ? targetKey(selected) : ""} disabled={disabled}
        onChange={(event) => onChoose(event.target.value)}>
        <option value="">{placeholder}</option>
        {stale && <option value="__stale" disabled>Previously selected session unavailable or changed</option>}
        {opened.length > 0 && <optgroup label="This feature">{opened.map(option)}</optgroup>}
        {companionNames.map((name) => (
          <optgroup key={name} label={`Triage companion: ${name}`}>{companions.filter((t) => t.feature_name === name).map(option)}</optgroup>
        ))}
      </select>
      {selected?.destination_kind === "companion" && <span className="muted small"> Companion feature {selected.feature_name}. Nothing is sent until you confirm.</span>}
    </label>
  );
}
