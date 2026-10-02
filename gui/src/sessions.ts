import { Feature, FeatureSession } from "./api";

// A session with a tmux window right now. A Todos session never has one, and a stopped session has lost its own.
function isLive(session: FeatureSession, stoppedSessionIds: string[]): boolean {
  return session.kind !== "todos" && !stoppedSessionIds.includes(session.id);
}

// Whether closing this session takes its running feature down with it. The backend stops the feature outright
// when it is the only session, whatever its kind; otherwise killing the last live tmux window ends the tmux
// session.
export function closingStopsFeature(feature: Feature, sessionId: string, stoppedSessionIds: string[]): boolean {
  if (feature.status === "stopped") return false;
  if (feature.sessions.length === 1) return true;
  return feature.sessions.some((session) => session.id === sessionId && isLive(session, stoppedSessionIds))
    && !feature.sessions.some((session) => session.id !== sessionId && isLive(session, stoppedSessionIds));
}
