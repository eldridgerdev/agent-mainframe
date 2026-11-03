import { HarnessInfo, asGuiError } from "./api";
import { Field, Spinner } from "./ui";
import { useCreationHarness } from "./useCreationHarness";

export default function CreationHarnessField({ harness, supported, disabled }: {
  harness: ReturnType<typeof useCreationHarness>; supported: HarnessInfo[]; disabled: boolean;
}) {
  const { choices, agent, offered, enabled } = harness;
  return <div>
    {enabled && (choices.isPending || choices.isFetching) && <p role="status"><Spinner /> Loading the project's default agent…</p>}
    {enabled && choices.isError && <p role="alert" className="callout callout-danger">Could not load the project's agents: {asGuiError(choices.error).message}
      <button type="button" disabled={disabled || choices.isFetching} onClick={() => void choices.refetch()}>Retry</button></p>}
    {enabled && choices.isSuccess && !offered.length && <p role="alert" className="callout callout-warning">No allowed, installed agents are available for this project.</p>}
    {enabled && agent && choices.isSuccess && !offered.includes(agent) && <p role="alert" className="callout callout-warning">The selected agent {agent} is unavailable for this project. Choose another agent.</p>}
    <Field label="Agent"><select value={agent ?? ""} disabled={disabled || !enabled || !choices.isSuccess || choices.isFetching || !offered.length}
      onChange={event => harness.choose(event.target.value as HarnessInfo["slug"])}>
      {!agent && <option value="" disabled>{enabled ? "Choose an agent…" : "Choose a project first…"}</option>}
      {agent && !offered.includes(agent) && <option value={agent} disabled>{agent} — unavailable</option>}
      {offered.map(slug => <option key={slug} value={slug}>{supported.find(info => info.slug === slug)?.display_name ?? slug}</option>)}
    </select></Field>
    {enabled && choices.isSuccess && <button type="button" className="btn btn-ghost" disabled={disabled || choices.isFetching}
      onClick={() => void choices.refetch()}>Reload agents</button>}
  </div>;
}
