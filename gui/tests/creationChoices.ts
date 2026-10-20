import type { AgentSlug, ProjectHarnessChoices } from "../src/api";
export function creationChoices(agents: AgentSlug[], preferred: AgentSlug = agents[0] ?? "claude"): ProjectHarnessChoices {
  return { project_id: "project", project_name: "demo", repo: "/tmp/demo", settings: { preferred_agent: preferred },
    agents, default_agent: agents.includes(preferred) ? preferred : agents[0] ?? null, revision: "choices-rev" };
}
