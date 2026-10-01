import type { Feature, FeatureSession } from "./api";
import { Icon, Spinner, StatusDot } from "./ui";

/** A session has a live terminal only while its feature runs and it was not
 *  stopped on its own. TODOs have no terminal and never count. */
export function sessionRunning(feature: Feature, session: FeatureSession): boolean {
  return feature.status !== "stopped" && session.kind !== "todos" && !session.stopped;
}

/** How many of a feature's sessions were stopped on their own. */
export function stoppedSessionCount(feature: Feature): number {
  return feature.sessions.filter((session) => session.stopped).length;
}

export function SessionStateDot({ running }: { running: boolean }) {
  return <StatusDot status={running ? "active" : "stopped"} />;
}

/** Start or stop one session, leaving the rest of the feature alone. */
export function SessionStartStopButton({
  label,
  running,
  starting,
  stopping,
  onStart,
  onStop,
}: {
  label: string;
  running: boolean;
  starting: boolean;
  stopping: boolean;
  onStart: () => void;
  onStop: () => void;
}) {
  return running ? (
    <button
      className="btn btn-ghost btn-sm"
      onClick={onStop}
      disabled={stopping}
      aria-label={`Stop session ${label}`}
      title="Stop this session and keep it listed"
    >
      {stopping ? <Spinner /> : <Icon name="stop" size={12} />}
      {stopping ? "Stopping…" : "Stop session"}
    </button>
  ) : (
    <button
      className="btn btn-secondary btn-sm"
      onClick={onStart}
      disabled={starting}
      aria-label={`Start session ${label}`}
    >
      {starting ? <Spinner /> : <Icon name="play" size={12} />}
      {starting ? "Starting…" : "Start session"}
    </button>
  );
}
