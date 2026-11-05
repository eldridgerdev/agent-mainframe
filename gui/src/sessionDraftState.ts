import { useEffect, useState, type Dispatch, type SetStateAction } from "react";
import type { WorkspaceSnapshot } from "./api";

type SessionDraftState = {
  drafts: Record<string, string>;
  setDrafts: Dispatch<SetStateAction<Record<string, string>>>;
  sendingPrompts: Record<string, boolean>;
  setSendingPrompts: Dispatch<SetStateAction<Record<string, boolean>>>;
  composerFocusKey: string | null;
  setComposerFocusKey: Dispatch<SetStateAction<string | null>>;
};

function pruneKeys<T>(current: Record<string, T>, live: Set<string>): Record<string, T> {
  const stale = Object.keys(current).filter((key) => !live.has(key));
  if (stale.length === 0) return current;
  const next = { ...current };
  for (const key of stale) delete next[key];
  return next;
}

export function useSessionDraftState(
  workspace: WorkspaceSnapshot | undefined,
  pendingSessionByFeature: Record<string, string>,
): SessionDraftState {
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [sendingPrompts, setSendingPrompts] = useState<Record<string, boolean>>({});
  const [composerFocusKey, setComposerFocusKey] = useState<string | null>(null);

  // Preserve a newly launched session's handoff draft until the snapshot catches up.
  useEffect(() => {
    if (!workspace) return;
    const live = new Set(workspace.projects.flatMap((project) => project.features.flatMap(
      (feature) => feature.sessions.map((session) => `${feature.id}:${session.id}`))));
    for (const [featureId, sessionId] of Object.entries(pendingSessionByFeature)) {
      live.add(`${featureId}:${sessionId}`);
    }
    setDrafts((current) => pruneKeys(current, live));
    setSendingPrompts((current) => pruneKeys(current, live));
    setComposerFocusKey((current) => current !== null && !live.has(current) ? null : current);
  }, [workspace, pendingSessionByFeature]);

  return { drafts, setDrafts, sendingPrompts, setSendingPrompts, composerFocusKey, setComposerFocusKey };
}
