// Inline confirmations render as `role="alert"` regions marked with
// `data-confirm-callout` (see ConfirmCallout in src/ui.tsx). Error messages are
// alerts too, so tests find "the confirmation" by its marker, not its role.
import { CONFIRM_CALLOUT_SELECTOR } from "../src/ui";

/** The inline confirmation inside `root`, or null. */
export function queryConfirm(root: ParentNode = document): HTMLElement | null {
  return root.querySelector<HTMLElement>(CONFIRM_CALLOUT_SELECTOR);
}

/** The inline confirmation inside `root`; throws when there is none. */
export function getConfirm(root: ParentNode = document): HTMLElement {
  const found = queryConfirm(root);
  if (!found) throw new Error("No inline confirmation is shown");
  return found;
}
