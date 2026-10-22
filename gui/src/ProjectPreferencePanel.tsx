import { useState } from "react";
import { AgentSlug, HarnessInfo, ProjectHarnessChoices } from "./api";
import RevisionedSettingsModal from "./RevisionedSettingsModal";
import { Field } from "./ui";

export default function ProjectPreferencePanel({ projectId, supported, onClose, onSaved }: {
  projectId: string; supported: HarnessInfo[]; onClose: () => void; onSaved: () => void;
}) {
  // The same labels the creation forms show.
  const name = (slug: AgentSlug) => supported.find(info => info.slug === slug)?.display_name ?? slug;
  const [agent, setAgent] = useState<AgentSlug | null>(null);
  const [offered, setOffered] = useState<AgentSlug[]>([]);
  return <RevisionedSettingsModal<ProjectHarnessChoices["settings"], ProjectHarnessChoices>
    label="Project preferred harness"
    subtitle={view => `${view?.project_name ?? "Project"} · Default for new features; existing sessions stay unchanged.`}
    loadCommand="project_harness_choices" saveCommand="project_preference_save" args={{ projectId }}
    onLoaded={view => { setAgent(view.settings.preferred_agent); setOffered(view.agents); }}
    draft={agent && offered.includes(agent) ? { preferred_agent: agent } : null}
    isDirty={loaded => agent !== loaded.preferred_agent} onClose={onClose} onSaved={onSaved}>
    {(busy, view) => <>
      <p className="muted small">Sets the default for new features. Existing sessions keep their harness. Agent availability is saved separately. Nothing starts or stops.</p>
      <Field label="Preferred harness"><select value={agent ?? ""} disabled={busy || !view.agents.length}
        onChange={event => setAgent(event.target.value as AgentSlug)}>
        {agent && !view.agents.includes(agent) && <option value={agent} disabled>{name(agent)} — unavailable</option>}
        {view.agents.map(choice => <option key={choice} value={choice}>{name(choice)}</option>)}
      </select></Field>
      {!view.agents.length && <p role="alert" className="callout callout-warning">No allowed, installed harness is available. Update Agent availability or install a harness, then reload.</p>}
      {agent && !view.agents.includes(agent) && <p role="alert" className="callout callout-warning">The saved preference is unavailable. New features default to {view.default_agent ? name(view.default_agent) : "no harness"}; choose an available harness to update the preference.</p>}
    </>}
  </RevisionedSettingsModal>;
}
