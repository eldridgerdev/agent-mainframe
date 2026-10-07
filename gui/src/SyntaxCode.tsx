import { Fragment, memo, useEffect, useRef, useState } from "react";
import { asGuiError } from "./api";
import { SyntaxInfo, SyntaxInstallView, SyntaxSpan, syntaxInstall, syntaxInstallStatus } from "./syntaxApi";
import { Spinner } from "./ui";
import "./syntax.css";

/**
 * One line of code, coloured by the backend's token spans when it sent any
 * and plain otherwise. Memoized: the spans arrive with the view, so a
 * selection or draft change re-renders rows without rebuilding their tokens.
 */
export const SyntaxCode = memo(function SyntaxCode({ text, spans }: { text: string; spans?: SyntaxSpan[] | null }) {
  if (!spans || spans.length === 0) return <code>{text}</code>;
  return <code className="syntax">{spans.map(([token, part], index) => token
    ? <span key={index} className={`syn-${token}`}>{part}</span>
    : <Fragment key={index}>{part}</Fragment>)}</code>;
});

const PLAIN_REASON: Partial<Record<SyntaxInfo["status"], (language: string | null) => string>> = {
  unsupported: () => "no supported language",
  not_installed: (language) => `${language} parser not installed`,
  broken: (language) => `${language} parser needs repair`,
  too_large: () => "file too large to highlight",
  over_budget: () => "too many changes to highlight every file",
};

/**
 * Says how the shown file is coloured and, when its parser is missing or
 * broken, offers the install the TUI's syntax-language picker performs,
 * behind an explicit confirmation. `onInstalled` runs once when an install
 * this badge watched succeeds, so the view can reload with colours.
 */
export function SyntaxBadge({ info, onInstalled }: { info: SyntaxInfo | null | undefined; onInstalled?: () => void }) {
  const installable = (info?.status === "not_installed" || info?.status === "broken") && !!info.language_key;
  const [install, setInstall] = useState<SyntaxInstallView | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [requesting, setRequesting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const seen = useRef<number | null>(null);
  const installed = useRef(onInstalled);
  installed.current = onInstalled;

  useEffect(() => { setNotice(null); }, [info?.language_key]);

  // Pick up an install already running (started from another view).
  useEffect(() => {
    if (!installable) return;
    let cancelled = false;
    void syntaxInstallStatus().then((view) => { if (!cancelled) setInstall(view); }).catch(() => { /* No install state. */ });
    return () => { cancelled = true; };
  }, [installable, info?.language_key]);

  useEffect(() => {
    if (!install?.running) return;
    const timer = window.setInterval(() => {
      void syntaxInstallStatus().then(setInstall).catch(() => { /* Keep the last progress. */ });
    }, 500);
    return () => window.clearInterval(timer);
  }, [install?.running]);

  useEffect(() => {
    if (!install) return;
    if (seen.current !== null && install.completed > seen.current && install.message) {
      setNotice(install.message);
      installed.current?.();
    }
    seen.current = install.completed;
  }, [install?.completed]);

  if (!info || info.status === "binary") return null;
  async function start() {
    if (!info?.language_key || requesting) return;
    setRequesting(true);
    setError(null);
    try {
      const view = await syntaxInstall(info.language_key);
      setConfirming(false);
      setInstall(view);
    } catch (err) {
      setError(asGuiError(err).message);
    } finally {
      setRequesting(false);
    }
  }
  const running = install?.running ?? false;
  const reason = PLAIN_REASON[info.status]?.(info.language);
  const verb = info.status === "broken" ? "Repair" : "Install";
  return <span className="syntax-badge">
    {info.status === "highlighted"
      ? <span className="syntax-chip syntax-chip-on" title="Syntax highlighting uses the parser the TUI installed">{info.language}</span>
      : <span className="syntax-chip" title={reason ? `Shown as plain text: ${reason}` : undefined}>Plain text{reason ? ` · ${reason}` : ""}</span>}
    {installable && !running && !confirming && <button type="button" className="btn btn-ghost btn-sm"
      onClick={() => { setError(null); setConfirming(true); }}>{verb} {info.language} parser…</button>}
    {installable && confirming && !running && <span className="syntax-install callout callout-warning" role="group" aria-label={`${verb} ${info.language} parser`}>
      <p>{verb} the {info.language} parser? AMF clones its tree-sitter grammar from GitHub and compiles it with your C compiler (<code>cc</code>) into AMF's config directory. The TUI uses the same parser.</p>
      <button type="button" className="btn btn-secondary btn-sm" disabled={requesting} onClick={() => setConfirming(false)}>Cancel</button>
      <button type="button" className="btn btn-primary btn-sm" disabled={requesting} onClick={() => void start()}>{requesting && <Spinner />} {verb} parser</button>
    </span>}
    {running && <span className="syntax-install" role="status">
      <Spinner /> Installing the {install?.language} parser…
      {install?.output && <span className="syntax-install-output">{install.output}</span>}
    </span>}
    {installable && !running && install?.error && install.language_key === info.language_key && <span className="syntax-install" role="alert">
      {install.language} parser install failed: {install.error}
    </span>}
    {error && <span className="syntax-install" role="alert">{error}</span>}
    {notice && !running && <span className="syntax-install" role="status">{notice}</span>}
  </span>;
}
