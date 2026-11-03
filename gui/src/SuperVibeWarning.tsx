import { useEffect, useState, type ReactNode } from "react";
import type { ModeSlug } from "./api";
import { ConfirmCallout } from "./ui";

/** What SuperVibe gives up, for any agent (not only Claude). */
export const SUPERVIBE_RISK = "The agent can run any command without asking, and change or delete anything in this checkout.";

/** A mode field's hint: the mode's own description, plus a red note when
 * SuperVibe is chosen. `undefined` when there is nothing to say. */
export function modeHint(description: ReactNode | undefined, mode: ModeSlug | string): ReactNode | undefined {
  if (mode !== "supervibe") return description || undefined;
  return <>{description}<SuperVibeNote /></>;
}

export function SuperVibeNote() {
  return <span className="supervibe-note" data-supervibe-note="">SuperVibe skips every permission check. {SUPERVIBE_RISK}</span>;
}

/** Whether the create-time confirm is open. Any mode change (the field, a
 * preset) closes it, so switching away and back never reopens it unasked. */
export function useSuperVibeConfirm(mode: ModeSlug) {
  const [open, setOpen] = useState(false);
  useEffect(() => setOpen(false), [mode]);
  return [open && mode === "supervibe", setOpen] as const;
}

/** The TUI's "SuperVibe Mode" confirmation before a feature is created in
 * SuperVibe. Focus starts on the safe choice, as Enter there must not create. */
export function SuperVibeConfirm({ busy = false, onSafer, onConfirm }: {
  busy?: boolean;
  onSafer: () => void;
  onConfirm: () => void;
}) {
  return <ConfirmCallout label="Confirm SuperVibe" className="callout callout-danger supervibe-confirm">
    <strong>Create this feature in SuperVibe?</strong>
    <p>SuperVibe skips every permission check. {SUPERVIBE_RISK}</p>
    <div className="row">
      <button type="button" className="btn btn-secondary" data-autofocus="" disabled={busy} onClick={onSafer}>Choose a safer mode</button>
      <button type="button" className="btn btn-danger" disabled={busy} onClick={onConfirm}>Create in SuperVibe</button>
    </div>
  </ConfirmCallout>;
}
