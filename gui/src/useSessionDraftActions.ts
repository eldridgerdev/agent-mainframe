import { useCallback, useRef } from "react";
import type { Dispatch, SetStateAction } from "react";
import { terminalSubmitPrompt, type SessionTarget } from "./api";

type SessionDraftActionsOptions = {
  setDrafts: Dispatch<SetStateAction<Record<string, string>>>;
  setSendingPrompts: Dispatch<SetStateAction<Record<string, boolean>>>;
  onError: (error: unknown) => void;
  onSent: (target: SessionTarget) => void;
};

export const DRAFT_SEPARATOR = "\n\n";
const keyFor = (target: SessionTarget) => `${target.feature_id}:${target.session_id}`;

export function useSessionDraftActions({ setDrafts, setSendingPrompts, onError, onSent }: SessionDraftActionsOptions) {
  const inFlight = useRef(new Set<string>());

  const changeDraft = useCallback((target: SessionTarget, text: string) => {
    const key = keyFor(target);
    if (inFlight.current.has(key)) return;
    setDrafts((current) => ({ ...current, [key]: text }));
  }, [setDrafts]);

  const sendDraft = useCallback(async (target: SessionTarget, text: string) => {
    const key = keyFor(target);
    if (!text.trim() || inFlight.current.has(key)) return;
    inFlight.current.add(key);
    setSendingPrompts((current) => ({ ...current, [key]: true }));
    try {
      await terminalSubmitPrompt(target, text);
      onSent(target);
      // Keep only text appended by a handoff while this send was in flight.
      setDrafts((current) => {
        const draft = current[key];
        if (draft === undefined || !draft.startsWith(text)) return current;
        const rest = draft.slice(text.length);
        return { ...current, [key]: rest.startsWith(DRAFT_SEPARATOR) ? rest.slice(DRAFT_SEPARATOR.length) : rest };
      });
    } catch (error) {
      onError(error);
    } finally {
      inFlight.current.delete(key);
      setSendingPrompts((current) => {
        if (!(key in current)) return current;
        const next = { ...current };
        delete next[key];
        return next;
      });
    }
  }, [onError, onSent, setDrafts, setSendingPrompts]);

  return { changeDraft, sendDraft };
}
