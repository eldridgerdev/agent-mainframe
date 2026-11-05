import { useCallback, useEffect, useState } from "react";
import type { Project, SessionTarget } from "./api";

const PENDING_SESSION_GRACE_MS = 10_000;

export function usePendingSessionState(projects: Project[] | undefined) {
  const [pendingSessionByFeature, setPendingSessionByFeature] = useState<Record<string, string>>({});

  // Once the snapshot sees the session, it can be rendered from persisted state.
  useEffect(() => {
    if (!projects) return;
    setPendingSessionByFeature((current) => {
      const arrived = Object.entries(current).filter(([featureId, sessionId]) =>
        projects.some((project) => project.features.some((feature) =>
          feature.id === featureId && feature.sessions.some((session) => session.id === sessionId))));
      if (arrived.length === 0) return current;
      const next = { ...current };
      for (const [featureId] of arrived) delete next[featureId];
      return next;
    });
  }, [projects]);

  const markPending = useCallback((target: SessionTarget) => {
    setPendingSessionByFeature((current) => ({ ...current, [target.feature_id]: target.session_id }));
    window.setTimeout(() => setPendingSessionByFeature((current) => {
      if (current[target.feature_id] !== target.session_id) return current;
      const next = { ...current };
      delete next[target.feature_id];
      return next;
    }), PENDING_SESSION_GRACE_MS);
  }, []);

  return { pendingSessionByFeature, markPending };
}
