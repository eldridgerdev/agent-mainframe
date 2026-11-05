import { useEffect, type RefObject } from "react";
import { keyIsForOtherInput } from "./sidebarPrefs";

export function useWorkspaceFilterShortcut(inputRef: RefObject<HTMLInputElement>) {
  useEffect(() => {
    // '/' focuses the filter unless typing belongs to another input or dialog.
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "/" || event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey
          || keyIsForOtherInput(event)) return;
      const input = inputRef.current;
      if (!input || input.closest("[hidden]")) return;
      input.focus();
      if (document.activeElement !== input) return;
      event.preventDefault();
      input.select();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [inputRef]);
}
