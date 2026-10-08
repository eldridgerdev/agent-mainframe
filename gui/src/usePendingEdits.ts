import { useEffect, useRef } from "react";
import { useQuery } from "@tanstack/react-query";
import type { FeatureTarget } from "./api";
import type { Toast } from "./ui";
import { PendingEditCount, supervisedEditCounts } from "./supervisedEditsApi";

export const PENDING_EDITS_KEY = ["supervised-edit-counts"];

/** The backend orders features by their oldest waiting edit. Auto-opening
 * consumes a notice only, never an answer. Dismissed edits remain badged and
 * can be reopened manually; a successful answer exposes the next oldest id. */
export function usePendingEdits(
  pushToast: (toast: Omit<Toast, "id">) => void,
  onOpen: (target: FeatureTarget) => void,
  popup?: { blocked: boolean; draftPending?: boolean; activeTarget: FeatureTarget | null;
    onDeferred?: (reason: string | null) => void; onOpen: (entry: PendingEditCount) => void },
): Record<string, number> {
  const counts = useQuery({ queryKey: PENDING_EDITS_KEY, queryFn: supervisedEditCounts, refetchInterval: 2_000, retry: false });
  const latest = useRef({ popup, onOpen, entries: counts.data });
  latest.current = { popup, onOpen, entries: counts.data };
  const announced = useRef<Set<string> | null>(null);
  const opened = useRef(new Set<string>());

  useEffect(() => {
    if (!counts.data) return;
    const seen = announced.current;
    if (seen === null) {
      announced.current = new Set(counts.data.map((entry) => entry.first_id));
      return;
    }
    for (const entry of counts.data) {
      if (seen.has(entry.first_id)) continue;
      seen.add(entry.first_id);
      pushToast({
        tone: "info", title: "Edit waiting for review",
        message: `${entry.feature_name}: the agent wants to change ${entry.first_path}.`,
        action: { label: "Review", onClick: () => latest.current.onOpen({ project_id: entry.project_id, feature_id: entry.feature_id }) },
      });
    }
  }, [counts.data, pushToast]);

  useEffect(() => {
    if (!popup?.activeTarget) return;
    const current = counts.data?.find((entry) => entry.feature_id === popup.activeTarget?.feature_id
      && entry.project_id === popup.activeTarget.project_id);
    if (current) opened.current.add(current.first_id);
  }, [counts.data, popup?.activeTarget]);

  useEffect(() => {
    const check = () => {
      const current = latest.current;
      const next = current.entries?.find((entry) => !opened.current.has(entry.first_id));
      if (!current.popup) return;
      if (current.popup.blocked || !next) {
        current.popup.onDeferred?.(null);
        return;
      }
      // Forms own their unsaved state: saved nonempty values are not drafts.
      // Minimized workflows still protect unsaved work, but their hidden
      // dialogs do not block a popup once that work has been saved.
      const draft = current.popup.draftPending || document.querySelector('[data-unsaved-changes="true"]');
      const modal = Array.from(document.querySelectorAll("[role=dialog], [role=alertdialog], [role=menu]"))
        .some((element) => !element.closest('[hidden], [aria-hidden="true"]'));
      if (draft || modal) {
        current.popup.onDeferred?.(draft
          ? "Automatic review waits until you save or discard your draft."
          : "Automatic review waits until you close the dialog or menu.");
        return;
      }
      current.popup.onDeferred?.(null);
      opened.current.add(next.first_id);
      current.popup.onOpen(next);
    };
    const timer = window.setInterval(check, 250);
    return () => {
      window.clearInterval(timer);
    };
  }, []);
  return Object.fromEntries((counts.data ?? []).map((entry) => [entry.feature_id, entry.count]));
}
