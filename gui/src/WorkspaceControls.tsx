import type { Feature } from "./api";
import { Icon, Spinner } from "./ui";

export { PageHeader } from "./ui";

export interface Lifecycle {
  starting: boolean;
  stopping: boolean;
  onStart: () => void;
  onStop: () => void;
}

export function StartStopButton({ status, starting, stopping, onStart, onStop, size, title }: Lifecycle & {
  status: Feature["status"];
  size?: "sm";
  /** Tooltip on the Stop button, e.g. how many sessions it ends. */
  title?: string;
}) {
  // In lists every row has one of these, so they stay quiet; the feature
  // header's single Start is the primary action on its page.
  const list = size === "sm";
  return status === "stopped" ? (
    <button className={`btn btn-sm ${list ? "btn-secondary" : "btn-primary"}`} onClick={onStart} disabled={starting}>
      {starting ? <Spinner /> : <Icon name="play" size={14} />}
      {starting ? "Starting…" : "Start"}
    </button>
  ) : (
    <button className={`btn btn-sm ${list ? "btn-ghost" : "btn-secondary"}`} onClick={onStop} disabled={stopping} title={title}>
      {stopping ? <Spinner /> : <Icon name="stop" size={14} />}
      {stopping ? "Stopping…" : "Stop"}
    </button>
  );
}
