import { ReactNode, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Icon } from "./ui";

/** Whether the native file dialog can be reached. It needs the Tauri
 * runtime: a plain browser (`npm run dev` without Tauri) and jsdom tests have
 * none, and there the Browse… button is left out so typing stays the way in. */
export function nativeDialogAvailable(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** "Browse…" beside a path field: opens the system's folder or file picker
 * and hands the chosen path to `onPick`. Cancelling the picker calls nothing,
 * so the field keeps whatever was typed. If the picker fails (for example a
 * permission or desktop-portal problem) the reason is shown under the row and
 * the field still accepts a typed path; pressing Browse… again retries. */
export function BrowseButton({
  kind,
  label,
  dialogTitle,
  defaultPath,
  disabled,
  onPick,
}: {
  kind: "directory" | "file";
  /** Accessible name, e.g. "Browse for repository folder". */
  label: string;
  /** The native dialog's window title. */
  dialogTitle?: string;
  /** Where the picker opens; usually what is already typed. */
  defaultPath?: string;
  disabled?: boolean;
  onPick: (path: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  if (!nativeDialogAvailable()) return null;

  async function browse() {
    setBusy(true);
    setFailure(null);
    try {
      const picked = await open({
        directory: kind === "directory",
        multiple: false,
        title: dialogTitle,
        defaultPath: defaultPath?.trim() || undefined,
      });
      // `null` is a cancelled picker: leave the field alone.
      if (typeof picked === "string" && picked) onPick(picked);
    } catch (cause) {
      const reason = cause instanceof Error ? cause.message : String(cause);
      setFailure(`The ${kind === "directory" ? "folder" : "file"} picker could not open (${reason}). Type the path instead.`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <button
        type="button"
        className="btn btn-secondary path-browse"
        aria-label={label}
        disabled={disabled || busy}
        onClick={() => void browse()}
      >
        <Icon name={kind === "directory" ? "folder" : "file"} />
        Browse…
      </button>
      {failure && <span className="path-browse-error field-error" role="alert">{failure}</span>}
    </>
  );
}

/** A path input with its Browse… button on one row. */
export function PathInputRow({ children, browse }: { children: ReactNode; browse: ReactNode }) {
  return <div className="path-input-row">{children}{browse}</div>;
}
