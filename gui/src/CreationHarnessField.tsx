import { HarnessInfo, asGuiError } from "./api";
import { Field, Spinner } from "./ui";
import { useCreationHarness } from "./useCreationHarness";

export default function CreationHarnessField({ harness, supported, disabled }: {
  harness: ReturnType<typeof useCreationHarness>; supported: HarnessInfo[]; disabled: boolean;
}) {
  const { choices, agent, offered } = harness;
  return <div>
    {(choices.isPending || choices.isFetching) && <p role="status"><Spinner /> Loading project harness default…</p>}
    {choices.isError && <p role="alert" className="callout callout-danger">Could not load project harnesses: {asGuiError(choices.error).message}
      <button type="button" disabled={disabled || choices.isFetching} onClick={() => void choices.refetch()}>Retry harnesses</button></p>}
    {choices.isSuccess && !offered.length && <p role="alert" className="callout callout-warning">No allowed, installed harnesses are available for this project.</p>}
    {agent && choices.isSuccess && !offered.includes(agent) && <p role="alert" className="callout callout-warning">The selected harness {agent} is unavailable for this project. Choose another harness explicitly.</p>}
    <Field label="Harness"><select value={agent ?? ""} disabled={disabled || !choices.isSuccess || choices.isFetching || !offered.length}
      onChange={event => harness.choose(event.target.value as HarnessInfo["slug"])}>
      {!agent && <option value="" disabled>Choose a harness…</option>}
      {agent && !offered.includes(agent) && <option value={agent} disabled>{agent} — unavailable</option>}
      {offered.map(slug => <option key={slug} value={slug}>{supported.find(info => info.slug === slug)?.display_name ?? slug}</option>)}
    </select></Field>
    {choices.isSuccess && <button type="button" className="btn btn-ghost" disabled={disabled || choices.isFetching}
      onClick={() => void choices.refetch()}>Reload harnesses</button>}
  </div>;
}
