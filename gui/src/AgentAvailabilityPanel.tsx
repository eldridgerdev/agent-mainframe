import { useState } from "react";
import { AgentSlug } from "./api";
import RevisionedSettingsModal from "./RevisionedSettingsModal";

interface Settings { allowed_agents: AgentSlug[] }

const agents: [AgentSlug, string][] = [["claude", "Claude Code"], ["codex", "Codex"], ["opencode", "OpenCode"], ["pi", "Pi"]];

export default function AgentAvailabilityPanel({ onClose, onSaved }: {
  onClose: () => void; onSaved: () => void;
}) {
  const [allowed, setAllowed] = useState<AgentSlug[]>([]);
  const valid = allowed.length > 0;

  return <RevisionedSettingsModal<Settings> label="Agent availability"
    subtitle="Global choices shared with the TUI. Existing sessions keep running."
    loadCommand="agent_availability_load" saveCommand="agent_availability_save"
    onLoaded={(settings) => setAllowed(settings.allowed_agents)}
    draft={valid ? { allowed_agents: agents.map(([agent]) => agent).filter(agent => allowed.includes(agent)) } : null}
    isDirty={(loaded) => agents.some(([agent]) => allowed.includes(agent) !== loaded.allowed_agents.includes(agent))}
    onClose={onClose} onSaved={onSaved}>
    {(busy) => <>
      {agents.map(([agent, label]) => <label key={agent} className={`option ${allowed.includes(agent) ? "option-selected" : ""}`}>
        <input type="checkbox" checked={allowed.includes(agent)} disabled={busy}
          onChange={(event) => setAllowed(current => event.target.checked ? [...current, agent] : current.filter(value => value !== agent))} /> {label}
      </label>)}
      <p className="muted">Choose the globally allowed agents for new features and sessions. Project allowed-agent settings take precedence.
        Installed harness availability also applies. This does not install or sign in to an agent.</p>
      <p className="muted">Saved changes apply here immediately. Restart an already open TUI: it keeps the choices it started with,
        and saving settings from it puts them back.</p>
      {!valid && <p role="alert">Select at least one agent.</p>}
    </>}
  </RevisionedSettingsModal>;
}
