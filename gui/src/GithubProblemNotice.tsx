import { useState } from "react";
import { asGuiError, type GithubProblem } from "./api";
import { openScreenshotBrowser } from "./screenshotsApi";
import { Spinner } from "./ui";
import "./githubProblem.css";

/** A GitHub failure the user can fix: what is wrong, what to run or install,
 * and Retry. The original `gh` text stays one click away under Details, so
 * nothing is hidden, only moved out of the headline. Never a dead end. */
export default function GithubProblemNotice({ problem, retrying, onRetry, retryLabel = "Retry" }: {
  problem: GithubProblem;
  retrying?: boolean;
  /** Omit when the panel has nothing to re-read (the message still says what to do). */
  onRetry?: () => void;
  retryLabel?: string;
}) {
  const [linkError, setLinkError] = useState<string | null>(null);
  return <div role="alert" className="callout callout-warning github-problem-notice" data-github-problem={problem.kind}>
    <p><strong>{problem.title}</strong></p>
    <p>{problem.message}</p>
    {problem.command && <pre className="github-problem-command" aria-label="Command to run in a terminal"><code>{problem.command}</code></pre>}
    {/* The backend's step stops short of Retry: only a panel that offers it says so. */}
    <p>{onRetry ? `Then press ${retryLabel}.` : "Then try again."}</p>
    {problem.link && <p className="small">
      <a href={problem.link} onClick={(event) => {
        event.preventDefault();
        setLinkError(null);
        void openScreenshotBrowser(problem.link!).catch((cause) => setLinkError(asGuiError(cause).message));
      }}>{problem.link.replace(/^https?:\/\//, "")}</a>
    </p>}
    {linkError && <p className="small">Could not open the browser: {linkError}</p>}
    {problem.detail && <details className="small">
      <summary>Details</summary>
      <pre className="github-problem-detail">{problem.detail}</pre>
    </details>}
    {onRetry && <div className="row">
      <button type="button" className="btn btn-secondary btn-sm" disabled={retrying} onClick={onRetry}>
        {retrying && <Spinner />} {retryLabel}
      </button>
    </div>}
  </div>;
}
