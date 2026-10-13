import { useState } from "react";
import { AgentSlug } from "./api";
import RevisionedSettingsModal, { RevisionedView } from "./RevisionedSettingsModal";

/** `null` is a project inheriting the global choices; the global setting
 * itself is always a list. */
interface Settings { allowed_agents: AgentSlug[] | null }

/** The project fields are present only for a project's settings. */
interface View extends RevisionedView<Settings> {
  inherited_agents?: AgentSlug[];
  project_name?: string;
  repo?: string;
  config_path?: string;
  source_path?: string | null;
}

const agents: [AgentSlug, string][] = [["claude", "Claude Code"], ["codex", "Codex"], ["opencode", "OpenCode"], ["pi", "Pi"]];

export default function AgentAvailabilityPanel({ projectId, onClose, onSaved }: {
  projectId?: string; onClose: () => void; onSaved: () => void;
}) {
  const [allowed, setAllowed] = useState<AgentSlug[]>([]);
  const [inherit, setInherit] = useState(false);
  const valid = inherit || allowed.length > 0;

  return <RevisionedSettingsModal<Settings, View> label={projectId ? "Project agent availability" : "Agent availability"}
    subtitle={(view) => projectId
      ? `${view?.project_name ?? "Project"} · Choices shared with the TUI. Existing sessions keep running.`
      : "Global choices shared with the TUI. Existing sessions keep running."}
    loadCommand={projectId ? "project_agent_availability_load" : "agent_availability_load"}
    saveCommand={projectId ? "project_agent_availability_save" : "agent_availability_save"}
    args={projectId ? { projectId } : undefined}
    onLoaded={(view) => {
      setInherit(view.settings.allowed_agents === null);
      setAllowed(view.settings.allowed_agents ?? view.inherited_agents ?? []);
    }}
    draft={valid ? { allowed_agents: inherit ? null : agents.map(([agent]) => agent).filter(agent => allowed.includes(agent)) } : null}
    isDirty={(loaded) => inherit !== (loaded.allowed_agents === null) ||
      (!inherit && agents.some(([agent]) => allowed.includes(agent) !== (loaded.allowed_agents ?? []).includes(agent)))}
    onClose={onClose} onSaved={onSaved}>
    {(busy, view) => {
      const shown = inherit ? (view.inherited_agents ?? []) : allowed;
      return <>
        {projectId && <>
          <p className="mono muted">{view.repo}</p>
          <label className={`option ${inherit ? "option-selected" : ""}`}>
            <input type="radio" name="agent-availability-scope" checked={inherit} disabled={busy}
              onChange={() => setInherit(true)} /> Use global setting (clear project override)
          </label>
          <label className={`option ${!inherit ? "option-selected" : ""}`}>
            <input type="radio" name="agent-availability-scope" checked={!inherit} disabled={busy}
              onChange={() => setInherit(false)} /> Use project override
          </label>
          <p className="muted">Global choices: {agents.filter(([agent]) => view.inherited_agents?.includes(agent)).map(([, label]) => label).join(", ")}</p>
        </>}
        {agents.map(([agent, label]) => <label key={agent} className={`option ${shown.includes(agent) ? "option-selected" : ""}`}>
          <input type="checkbox" checked={shown.includes(agent)} disabled={busy || inherit}
            onChange={(event) => setAllowed(current => event.target.checked ? [...current, agent] : current.filter(value => value !== agent))} /> {label}
        </label>)}
        <p className="muted">{projectId
          ? "Project choices apply to new features and sessions in this repository. Save with Use global setting to remove this project's override."
          : "Choose the globally allowed agents for new features and sessions. Project allowed-agent settings take precedence."}
          {" "}Installed harness availability also applies. This does not install or sign in to an agent.</p>
        {projectId
          ? <p className="muted">Saves to <span className="mono">{view.config_path}</span>.
            {view.source_path && view.source_path !== view.config_path && " The effective legacy config is copied to this file; the legacy file is kept unchanged."}</p>
          : <p className="muted">Saved changes apply here immediately. Restart an already open TUI: it keeps the choices it started with,
            and saving settings from it puts them back.</p>}
        {!valid && <p role="alert">Select at least one agent.</p>}
      </>;
    }}
  </RevisionedSettingsModal>;
}
