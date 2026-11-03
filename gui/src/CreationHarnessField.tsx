import { HarnessInfo, asGuiError } from "./api";
import AgentSetupNotice from "./AgentSetupNotice";
import { Field, Spinner } from "./ui";
import { useCreationHarness } from "./useCreationHarness";

type Harness = ReturnType<typeof useCreationHarness>;

/** The Agent label, select and reload action: one grid cell, so a form can
 * set it beside Mode with both labels on one baseline. */
export function CreationHarnessSelect({ harness, supported, disabled }: {
  harness: Harness; supported: HarnessInfo[]; disabled: boolean;
}) {
  const { choices, agent, offered, enabled } = harness;
  return <div className="creation-agent-cell">
    <Field label="Agent"><select value={agent ?? ""} disabled={disabled || !enabled || !choices.isSuccess || choices.isFetching || !offered.length}
      onChange={event => harness.choose(event.target.value as HarnessInfo["slug"])}>
      {!agent && <option value="" disabled>{enabled ? "Choose an agent…" : "Choose a project first…"}</option>}
      {agent && !offered.includes(agent) && <option value={agent} disabled>{agent} — unavailable</option>}
      {offered.map(slug => <option key={slug} value={slug}>{supported.find(info => info.slug === slug)?.display_name ?? slug}</option>)}
    </select></Field>
    {enabled && choices.isSuccess && offered.length > 0 && <button type="button" className="btn btn-ghost btn-sm" disabled={disabled || choices.isFetching}
      onClick={() => void choices.refetch()}>Reload agents</button>}
  </div>;
}

/** Loading, failure and unavailable states for the agent choice. Rendered
 * below the Agent | Mode row, full width, so they never push a label down. */
export function CreationHarnessNotices({ harness, disabled, onOpenAvailability }: {
  harness: Harness; disabled: boolean; onOpenAvailability?: () => void;
}) {
  const { choices, agent, offered, enabled } = harness;
  return <div className="creation-agent-notices">
    {enabled && (choices.isPending || choices.isFetching) && <p role="status"><Spinner /> Loading the project's default agent…</p>}
    {enabled && choices.isError && <p role="alert" className="callout callout-danger">Could not load the project's agents: {asGuiError(choices.error).message}
      <button type="button" disabled={disabled || choices.isFetching} onClick={() => void choices.refetch()}>Retry</button></p>}
    {enabled && choices.isSuccess && !offered.length && <AgentSetupNotice checking={choices.isFetching}
      onCheckAgain={() => void choices.refetch()} onOpenAvailability={onOpenAvailability} />}
    {enabled && agent && choices.isSuccess && !offered.includes(agent) && <p role="alert" className="callout callout-warning">The selected agent {agent} is unavailable for this project. Choose another agent.</p>}
  </div>;
}
