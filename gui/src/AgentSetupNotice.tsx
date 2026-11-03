import { useState } from "react";
import { asGuiError } from "./api";
import { openScreenshotBrowser } from "./screenshotsApi";
import { Spinner } from "./ui";

/** Where to get each agent AMF can run. Opened in the system browser. */
export const AGENT_INSTALL_LINKS: [string, string][] = [
  ["Claude Code", "https://docs.anthropic.com/en/docs/claude-code"],
  ["Codex", "https://github.com/openai/codex"],
  ["OpenCode", "https://opencode.ai"],
  ["Pi", "https://github.com/badlogic/pi-mono"],
];

/** What a new user sees when AMF has no agent it can start: what an agent is,
 * where to get one, and the two ways forward (allow one, or check again after
 * installing). Never a dead end. */
export default function AgentSetupNotice({ checking, onCheckAgain, onOpenAvailability }: {
  checking?: boolean;
  onCheckAgain: () => void;
  onOpenAvailability?: () => void;
}) {
  const [linkError, setLinkError] = useState<string | null>(null);
  return <div role="alert" className="callout callout-warning agent-setup-notice">
    <p><strong>No agent is available yet.</strong></p>
    <p>
      AMF runs AI coding agents such as Claude Code, Codex, OpenCode or Pi. None is installed and allowed on
      this computer. Install one, or allow an installed one in Agent availability, then press Check again.
    </p>
    <ul className="agent-setup-links" aria-label="Install an agent">
      {AGENT_INSTALL_LINKS.map(([name, url]) => <li key={name}>
        <a href={url} onClick={(event) => {
          event.preventDefault();
          setLinkError(null);
          void openScreenshotBrowser(url).catch((cause) => setLinkError(asGuiError(cause).message));
        }}>{name}</a>
      </li>)}
    </ul>
    {linkError && <p className="small">Could not open the browser: {linkError}</p>}
    <div className="row">
      <button type="button" className="btn btn-secondary btn-sm" disabled={checking} onClick={onCheckAgain}>
        {checking && <Spinner />} Check again
      </button>
      {onOpenAvailability && <button type="button" className="btn btn-ghost btn-sm" onClick={onOpenAvailability}>
        Agent availability…
      </button>}
    </div>
  </div>;
}
