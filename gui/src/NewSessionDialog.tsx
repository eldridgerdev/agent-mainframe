import { useState } from "react";
import type { NewSessionKind } from "./api";
import type { CustomSessionOption, NewSessionOptions, SessionChoice } from "./sessionsApi";
import { Icon, Modal, Spinner } from "./ui";
import "./sessions.css";

/** A custom session's `pre_check` refused it; nothing was created. */
export interface PreCheckFailure {
  name: string;
  preCheck: string;
  output: string;
}

type Key = string;
const builtinKey = (kind: NewSessionKind): Key => `builtin:${kind}`;
const customKey = (name: string): Key => `custom:${name}`;

/** The TUI's custom-session icon: the Nerd Font glyph, else the plain text. */
export function CustomSessionIcon({ option }: { option: Pick<CustomSessionOption, "icon" | "icon_nerd"> }) {
  const glyph = option.icon_nerd ?? null;
  return (
    <span className={glyph ? "custom-session-icon custom-session-glyph" : "custom-session-icon"} aria-hidden="true">
      {glyph ?? (option.icon ? option.icon.slice(0, 2) : "$")}
    </span>
  );
}

function CustomDetails({ option }: { option: CustomSessionOption }) {
  const rows: [string, string][] = [];
  if (option.command) rows.push(["Runs", option.command]);
  if (option.working_dir) rows.push(["In", option.working_dir]);
  if (option.pre_check) rows.push(["Pre-check", option.pre_check]);
  if (option.on_stop) rows.push(["On stop", option.on_stop]);
  return (
    <dl className="custom-session-details">
      {rows.map(([term, value]) => (
        <div key={term}><dt>{term}</dt><dd className="mono">{value}</dd></div>
      ))}
      {!option.command && <div><dt>Runs</dt><dd>a shell</dd></div>}
    </dl>
  );
}

export default function NewSessionDialog({
  options,
  preferredKind,
  busy,
  preCheckFailure,
  onCreate,
  onClose,
}: {
  options: NewSessionOptions;
  preferredKind: NewSessionKind;
  busy: boolean;
  /** Shown until the selection changes; the dialog stays open for it. */
  preCheckFailure: PreCheckFailure | null;
  onCreate: (choice: SessionChoice, label: string | null) => void;
  onClose: () => void;
}) {
  const enabled = options.builtin.filter((option) => option.disabled === null);
  const [key, setKey] = useState<Key>(() =>
    enabled.some((option) => option.kind === preferredKind)
      ? builtinKey(preferredKind)
      : enabled[0] ? builtinKey(enabled[0].kind)
      : options.custom[0] ? customKey(options.custom[0].name) : "");
  const [label, setLabel] = useState("");
  const [dismissedFailure, setDismissedFailure] = useState<PreCheckFailure | null>(null);

  const builtin = options.builtin.find((option) => builtinKey(option.kind) === key && option.disabled === null);
  const custom = options.custom.find((option) => customKey(option.name) === key);
  const choice: SessionChoice | null = custom
    ? { type: "custom", name: custom.name, revision: custom.revision }
    : builtin ? { type: "builtin", kind: builtin.kind } : null;
  // VS Code opens an external window and TODOs has a fixed name: the TUI
  // asks neither for a name.
  const named = !(builtin && (builtin.kind === "vscode" || builtin.kind === "todos"));
  const failure = preCheckFailure && preCheckFailure !== dismissedFailure && custom?.name === preCheckFailure.name
    ? preCheckFailure : null;
  const startsFeature = options.feature_stopped && builtin?.kind !== "todos";

  const select = (next: Key) => {
    setKey(next);
    if (preCheckFailure) setDismissedFailure(preCheckFailure);
  };

  return (
    <Modal
      label="New session"
      title="New session"
      subtitle="Open another agent, terminal, editor or configured session in this feature."
      onClose={onClose}
      dismissable={!busy}
      size="lg"
      onSubmit={() => {
        if (!busy && choice) onCreate(choice, named ? label.trim() || null : null);
      }}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose} disabled={busy}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={busy || !choice}>
            {busy && <Spinner />}
            {builtin?.kind === "vscode" ? "Open VS Code" : failure ? "Run pre-check again" : "Create session"}
          </button>
        </>
      }
    >
      <fieldset className="new-session-options">
        <legend>Session type</legend>
        {options.builtin.map((option) => (
          <label key={option.kind}
            className={option.disabled ? "new-session-option new-session-option-disabled" : "new-session-option"}
            title={option.disabled ?? undefined}>
            <input type="radio" name="new-session-kind" value={option.kind}
              disabled={option.disabled !== null}
              checked={key === builtinKey(option.kind)} onChange={() => select(builtinKey(option.kind))} />
            <span>{option.label}</span>
            {option.disabled && <span className="new-session-disabled">{option.disabled}</span>}
          </label>
        ))}
      </fieldset>

      {(options.custom.length > 0 || options.config_warning) && (
        <fieldset className="custom-session-options">
          <legend>Configured sessions <span className="muted">(amf.json)</span></legend>
          {options.config_warning && (
            <div className="callout callout-warning" role="status">
              <Icon name="alert" size={14} />
              <p>{options.config_warning}</p>
            </div>
          )}
          {options.custom.map((option) => (
            <label key={option.name} className="custom-session-option">
              <input type="radio" name="new-session-kind" value={option.name}
                aria-label={option.name}
                checked={key === customKey(option.name)} onChange={() => select(customKey(option.name))} />
              <CustomSessionIcon option={option} />
              <span className="custom-session-body">
                <span className="custom-session-title">
                  <strong>{option.name}</strong>
                  {option.source === "global" && <span className="badge">global</span>}
                  {option.autolaunch && <span className="badge" title="autolaunch: opens as soon as it is created">opens on create</span>}
                </span>
                {option.description && <span className="muted">{option.description}</span>}
                <CustomDetails option={option} />
              </span>
            </label>
          ))}
        </fieldset>
      )}

      {failure && (
        <div className="callout callout-danger custom-session-failure" role="alert">
          <strong>{failure.name} was not created: its pre-check failed</strong>
          <code className="mono">{failure.preCheck}</code>
          <pre>{failure.output}</pre>
        </div>
      )}

      {builtin?.kind === "vscode" && (
        <p className="muted new-session-note">
          Opens this worktree in a new VS Code window. AMF tracks the window and closes it when the
          feature stops, unless VS Code handed the folder to a window AMF did not open.
        </p>
      )}
      {builtin?.kind === "todos" && (
        <p className="muted new-session-note">
          Adds the feature's TODOs session (the TUI tree shows it). The list is also always on the TODOs tab.
        </p>
      )}
      {startsFeature && choice && (
        <p className="muted new-session-note">This feature is stopped: this starts it, with its saved agents.</p>
      )}

      {named && (
        <label className="field">
          <span>Session name <span className="muted">(optional)</span></span>
          <input type="text" value={label} onChange={(event) => setLabel(event.target.value)}
            placeholder={custom ? custom.name : "Use the next default name"} maxLength={80} />
        </label>
      )}
    </Modal>
  );
}
