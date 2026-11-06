import type { RefObject } from "react";
import { useShortcut } from "./shortcuts";

export function useWorkspaceFilterShortcut(inputRef: RefObject<HTMLInputElement>) {
  useShortcut("sidebar.filter.focus", () => {
    const input = inputRef.current;
    if (!input || input.closest("[hidden]")) return false;
    input.focus();
    if (document.activeElement !== input) return false;
    input.select();
  });
}
