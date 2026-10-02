import type { Feature, FeatureSession } from "./api";
import { Icon, Spinner, StatusDot } from "./ui";

/** A session has a live terminal only while its feature runs and it was not
 *  stopped on its own. TODOs have no terminal and never count. Pass the
 *  snapshot's `stopped_session_ids` to also count a session whose window is
 *  gone without being flagged (a tab held back by another tab's resume, or a
 *  window closed outside AMF). */
export function sessionRunning(
  feature: Feature,
  session: FeatureSession,
  stoppedSessionIds: string[] = [],
): boolean {
  return feature.status !== "stopped" && session.kind !== "todos" && !session.stopped
    && !stoppedSessionIds.includes(session.id);
}

/** Whether closing this session takes its running feature down with it. The
 *  backend stops the feature outright when it is the only session, whatever
 *  its kind; otherwise killing the last running window ends the tmux session. */
export function closingStopsFeature(feature: Feature, sessionId: string, stoppedSessionIds: string[]): boolean {
  if (feature.status === "stopped") return false;
  if (feature.sessions.length === 1) return true;
  const running = (session: FeatureSession) => sessionRunning(feature, session, stoppedSessionIds);
  return feature.sessions.some((session) => session.id === sessionId && running(session))
    && !feature.sessions.some((session) => session.id !== sessionId && running(session));
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
