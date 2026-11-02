import { useCallback, useEffect, useRef, useState } from "react";
import { asGuiError } from "./api";
import { steeringAnalyze, SteeringAnalysis } from "./steeringApi";
import { Icon, Spinner } from "./ui";

/**
 * Advisory steering checklist shown beside an UNSENT composer draft. It only
 * reads the draft: it has no send action and never touches the terminal. The
 * checklist is bound to the exact text it analyzed, so an edited draft marks
 * it stale until it is checked again.
 */
export default function SteeringPanel({ text, onClose }: { text: string; onClose: () => void }) {
  const [result, setResult] = useState<{ text: string; analysis: SteeringAnalysis } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const seq = useRef(0);
  const latest = useRef(text);
  latest.current = text;

  const check = useCallback(async () => {
    const requested = latest.current;
    const mine = ++seq.current;
    setBusy(true);
    setError(null);
    try {
      const analysis = await steeringAnalyze(requested);
      if (mine === seq.current) setResult({ text: requested, analysis });
    } catch (err) {
      if (mine === seq.current) setError(asGuiError(err).message);
    } finally {
      if (mine === seq.current) setBusy(false);
    }
  }, []);

  useEffect(() => { void check(); }, [check]);

  const stale = result !== null && result.text !== text;
  const analysis = result?.analysis;
  const tone = !analysis ? "" : analysis.score >= 8 ? "good" : analysis.score >= 4 ? "warn" : "bad";

  return (
    <section className="steering-panel" aria-label="Steering coach" data-stale={stale ? "true" : "false"}>
      <header className="steering-head">
        <Icon name="sparkles" />
        <strong>Steering Coach</strong>
        {analysis && !stale && !analysis.blank && <span className={`steering-score steering-${tone}`}>Score: {analysis.score} / {analysis.max_score}</span>}
        <span className="muted small">Advisory only. Your draft stays unsent.</span>
        <span className="tabs-spacer" />
        <button className="btn btn-sm btn-ghost" onClick={() => void check()}>
          {busy ? <Spinner /> : null}{stale ? "Re-check draft" : "Check draft"}
        </button>
        <button className="btn btn-sm btn-ghost" aria-label="Close steering coach" onClick={onClose}>Close</button>
      </header>
      {error && <p className="callout callout-danger" role="alert">{error}</p>}
      {stale && (
        <p className="callout callout-warning steering-stale" role="status">
          The draft changed since this check. Re-check it so the checklist matches the text you send.
        </p>
      )}
      {analysis && (
        <div className={stale ? "steering-body is-stale" : "steering-body"}>
          <p className="steering-summary">{analysis.summary}</p>
          <h4 className="steering-subhead">Constraint checklist</h4>
          <ul className="steering-checks" aria-label="Constraint checklist">
            {analysis.checks.map((item) => (
              <li key={item.key} data-present={item.present ? "true" : "false"}>
                <span role="img" aria-label={item.present ? "covered" : "missing"} className="steering-mark">{item.present ? "[x]" : "[ ]"}</span>
                <strong>{item.label}</strong>
                <span className="muted"> - {item.detail}</span>
              </li>
            ))}
          </ul>
          <h4 className="steering-subhead">How to strengthen it</h4>
          {analysis.tips.length === 0 ? (
            <p className="small">Your draft covers the main steering constraints. Send it when the wording is precise enough for this repo.</p>
          ) : (
            <ul className="steering-tips">{analysis.tips.map((tip) => <li key={tip}>{tip}</li>)}</ul>
          )}
        </div>
      )}
    </section>
  );
}
