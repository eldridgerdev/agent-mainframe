import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { asGuiError, type FeatureTarget } from "./api";
import { Field, Modal, Spinner } from "./ui";

type Choice = "cherry_pick" | "push";
type Plan = { revision: string; choice: Choice; companion_name: string; source_name: string; companion_path: string; source_path: string; companion_branch: string; source_branch: string; base_sha: string; companion_head: string; source_head: string; commits: string[]; source_dirty: boolean; companion_dirty: boolean; remote_tip: string | null; remote_display: string | null; ignored_collisions: string[]; hidden_tracked: string[] };

export default function CompanionIntegrationDialog({ target, onClose }: { target: FeatureTarget; onClose: () => void }) {
  const [choice, setChoice] = useState<Choice>("cherry_pick");
  const [plan, setPlan] = useState<Plan | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef(false);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    let timer = 0;
    const poll = async () => {
      try {
        const status = await invoke<{ running: boolean; result: string | null; error: { message: string } | null }>("companion_integration_status", { target });
        if (!live.current || pending.current) return;
        setBusy(status.running);
        if (!status.running && status.result) setResult(status.result);
        if (!status.running && status.error) setError(status.error.message);
        if (status.running) timer = window.setTimeout(poll, 1000);
      } catch { /* Preview still reports any operational error. */ }
    };
    void poll();
    return () => { live.current = false; window.clearTimeout(timer); };
  }, [target.project_id, target.feature_id]);
  const preview = async () => {
    if (pending.current) return;
    pending.current = true; setBusy(true); setPlan(null); setError(null); setResult(null);
    try { const next = await invoke<Plan>("companion_integration_preview", { target, choice }); if (live.current) setPlan(next); }
    catch (e) { if (live.current) setError(asGuiError(e).message); }
    finally { pending.current = false; if (live.current) setBusy(false); }
  };
  const confirm = async () => {
    if (!plan || pending.current) return;
    pending.current = true; setBusy(true); setError(null);
    try { const message = await invoke<string>("companion_integration_confirm", { target, choice: plan.choice, revision: plan.revision }); if (live.current) { setPlan(null); setResult(message); } }
    catch (e) { if (live.current) { setPlan(null); setError(asGuiError(e).message); } }
    finally { pending.current = false; if (live.current) setBusy(false); }
  };
  return <Modal label="Integrate companion commits" title="Integrate companion commits" subtitle="Review the exact destination and commits before continuing." onClose={onClose} dismissable={!busy} size="lg" footer={<><button className="btn btn-secondary" disabled={busy} onClick={onClose}>Close</button><button className="btn btn-secondary" disabled={busy} onClick={() => void preview()}>{busy && <Spinner />} Preview integration</button>{plan && <button className="btn btn-primary" disabled={busy || (plan.choice === "cherry_pick" && (plan.source_dirty || plan.ignored_collisions.length > 0 || plan.hidden_tracked.length > 0))} onClick={() => void confirm()}>Confirm {plan.choice === "push" ? "push" : "cherry-pick"}</button>}</>}>
    <Field label="Integration method"><select value={choice} disabled={busy} onChange={event => { setChoice(event.target.value as Choice); setPlan(null); setError(null); setResult(null); }}><option value="cherry_pick">Cherry-pick into source checkout</option><option value="push">Push to source branch on origin</option></select></Field>
    {error && <p className="callout callout-danger" role="alert">{error}</p>}{result && <p className="callout callout-success" role="status">{result}</p>}
    {plan && <section aria-label="Integration preview"><p><strong>{plan.companion_name}</strong> ({plan.companion_branch}) → <strong>{plan.source_name}</strong> ({plan.source_branch})</p><p>Companion checkout: <code>{plan.companion_path}</code><br />Source checkout: <code>{plan.source_path}</code></p><p>Base: <code>{plan.base_sha}</code><br />Companion HEAD: <code>{plan.companion_head}</code><br />Source HEAD: <code>{plan.source_head}</code>{plan.choice === "push" && <><br />Origin destination: <code>{plan.remote_display}</code><br />Origin branch tip: {plan.remote_tip ? <code>{plan.remote_tip}</code> : "Not present; confirmation will create the branch."}</>}</p><p>{plan.commits.length} commit(s) to integrate:</p><ol>{plan.commits.map(commit => <li key={commit}><code>{commit}</code></li>)}</ol>{plan.companion_dirty && <p className="callout callout-warning">The companion checkout has uncommitted changes. They will not be included.</p>}{plan.ignored_collisions.length > 0 && <p className="callout callout-warning">Ignored paths would be overwritten: {plan.ignored_collisions.join(", ")}. Move them before cherry-picking.</p>}{plan.hidden_tracked.length > 0 && <p className="callout callout-warning">Tracked paths are marked assume-unchanged or skip-worktree in the source checkout, so local edits could be hidden: {plan.hidden_tracked.join(", ")}. Clear those flags before cherry-picking.</p>}{plan.source_dirty && <p className="callout callout-warning">The source checkout has uncommitted changes. Cherry-pick is unavailable until it is clean.</p>}<p>{plan.choice === "push" ? "Confirmation updates origin only if the previewed branch state is unchanged; an existing branch update must be fast-forward." : "Confirmation applies these commits to the source checkout. A conflict triggers an abort; any failed recovery is reported. Concurrent external Git operations are not locked by this preview."}</p></section>}
  </Modal>;
}
