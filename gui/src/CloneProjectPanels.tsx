import { useState } from "react";
import type { CloneCheck, CloneCheckStatus, CloneFix, CloneProgress } from "./api";

const STATUS_LABEL: Record<CloneCheckStatus, string> = { pass: "Passed", warn: "Warning", fail: "Failed" };

/** A fix is shown and copied, never run: AMF does not touch credentials. */
export function FixCommand({ fix }: { fix: CloneFix }) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(fix.command_line);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  }
  return (
    <div className="clone-fix">
      <span className="clone-fix-label">{fix.label}</span>
      <code className="mono clone-fix-command">{fix.command_line}</code>
      <button type="button" className="btn btn-ghost" onClick={() => void copy()}
        aria-label={`Copy command: ${fix.command_line}`}>
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}

export function PreflightChecklist({ checks, defaultBranch }: { checks: CloneCheck[]; defaultBranch: string | null }) {
  return (
    <div className="clone-preflight">
      <ul className="clone-checks" aria-label="Pre-flight checks">
        {checks.map((check) => (
          <li key={check.id} className={`clone-check clone-check-${check.status}`} data-status={check.status}>
            <span className="clone-check-status">{STATUS_LABEL[check.status]}</span>
            <span className="clone-check-message">{check.message}</span>
            {check.fix && <FixCommand fix={check.fix} />}
          </li>
        ))}
      </ul>
      {defaultBranch && <p className="field-hint">Default branch: <code className="mono">{defaultBranch}</code></p>}
    </div>
  );
}

export function CloneProgressView({ progress }: { progress: CloneProgress | null }) {
  const percent = progress?.percent ?? null;
  return (
    <div className="clone-progress" role="status">
      <p>{progress ? progress.detail || progress.phase : "Starting the clone…"}</p>
      <progress max={100} value={percent ?? undefined} aria-label="Clone progress" />
    </div>
  );
}

/** What a failed clone says and the user-run fixes it suggests. */
export function FailureNotice({ message, advice, fixes }: { message: string; advice: string | null; fixes: CloneFix[] }) {
  return (
    <div className="clone-failure" role="alert">
      <p className="field-error"><strong>Could not clone the repository.</strong> {message}</p>
      {advice && <p className="field-hint">{advice}</p>}
      {fixes.map((fix) => <FixCommand key={fix.id} fix={fix} />)}
    </div>
  );
}
