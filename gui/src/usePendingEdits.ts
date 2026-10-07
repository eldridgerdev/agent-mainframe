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
  popup?: { blocked: boolean; activeTarget: FeatureTarget | null; onOpen: (entry: PendingEditCount) => void },
): Record<string, number> {
  const counts = useQuery({ queryKey: PENDING_EDITS_KEY, queryFn: supervisedEditCounts, refetchInterval: 2_000, retry: false });
  const latest = useRef({ popup, onOpen, entries: counts.data });
  latest.current = { popup, onOpen, entries: counts.data };
  const announced = useRef<Set<string> | null>(null);
  const opened = useRef(new Set<string>());
  const dirty = useRef(new Set<HTMLInputElement | HTMLTextAreaElement>());

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
    const changed = (event: Event) => {
      const field = event.target;
      // xterm's hidden textarea carries terminal input, not an unsaved form.
      if (!(field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement)
        || field.closest(".xterm")
        || (field instanceof HTMLInputElement && !["text", "search", "email", "url", "number", "password"].includes(field.type))) return;
      if (field.value) dirty.current.add(field);
      else dirty.current.delete(field);
    };
    const check = () => {
      const current = latest.current;
      if (!current.popup || current.popup.blocked || document.querySelector("[role=dialog], [role=alertdialog], [role=menu]")) return;
      for (const field of dirty.current) {
        if (!field.isConnected || !field.value) dirty.current.delete(field);
      }
      if (dirty.current.size > 0) return;
      const next = current.entries?.find((entry) => !opened.current.has(entry.first_id));
      if (!next) return;
      opened.current.add(next.first_id);
      current.popup.onOpen(next);
    };
    document.addEventListener("input", changed, true);
    document.addEventListener("change", changed, true);
    const timer = window.setInterval(check, 250);
    return () => {
      document.removeEventListener("input", changed, true);
      document.removeEventListener("change", changed, true);
      window.clearInterval(timer);
    };
  }, []);
  return Object.fromEntries((counts.data ?? []).map((entry) => [entry.feature_id, entry.count]));
}
