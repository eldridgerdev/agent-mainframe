import { useCallback, useEffect, useRef, useState } from "react";
import { AgentSlug, FeatureTarget, asGuiError } from "./api";
import Markdown from "./Markdown";
import PrMarkdown, { PrDescription, OpenPrImage } from "./PrMarkdown";
import ScreenshotViewer from "./ScreenshotViewer";
import type { ImageData } from "./screenshotsApi";
import {
  PrComment, PrReplyKind, PrSort, PrTriageAction, PrTriageView, prTriageAct, prTriageBegin, prTriageSnapshot,
} from "./prTriageApi";
import { Field, Modal, Spinner } from "./ui";

const SORTS: [PrSort, string][] = [
  ["fetch_order", "Fetch order"], ["by_file", "By file"], ["by_author", "By author"],
  ["humans_first", "Humans first"], ["conversations", "Conversations last"],
];

const REPLY_LABELS: Record<PrReplyKind, string> = {
  done: "Reply: fixed", not_needed: "Reply: not needed", investigation: "Reply with findings",
};

function location(comment: PrComment) {
  if (!comment.path) return comment.kind === "review_summary" ? `Review (${comment.review_state?.toLowerCase().replace("_", " ")})` : "Conversation";
  if (comment.file_level || comment.line === null) return comment.path;
  return `${comment.path}:${comment.line}${comment.side === "LEFT" ? " (base)" : ""}`;
}

function hunkClass(line: string) {
  if (line.startsWith("@@")) return "pr-hunk-header";
  if (line.startsWith("+")) return "diff-added";
  if (line.startsWith("-")) return "diff-removed";
  return "";
}

/** PR Triage: browse a pull request's review feedback and act on it. Every
 * GitHub write and AI call is shown first and needs an explicit confirmation. */
export default function PrTriagePanel({ target, onClose }: { target: FeatureTarget; onClose: () => void }) {
  const [view, setView] = useState<PrTriageView | null>(null);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [numberDraft, setNumberDraft] = useState("");
  const [investigate, setInvestigate] = useState<{ commentId: number; harness: AgentSlug; text: string; followUp: boolean } | null>(null);
  const [reply, setReply] = useState<{ key: string; text: string; seed: string } | null>(null);
  const [openImage, setOpenImage] = useState<{ image: ImageData; caption: string; identity: string } | null>(null);
  const imageTrigger = useRef<HTMLElement | null>(null);
  const [confirmClose, setConfirmClose] = useState(false);
  const pending = useRef(false);
  // Bring a newly opened editor into the detail pane's view.
  const reveal = useCallback((node: HTMLElement | null) => { node?.scrollIntoView?.({ block: "nearest" }); }, []);

  useEffect(() => {
    let live = true;
    prTriageBegin(target).then((next) => { if (live) setView(next); })
      .catch((err) => { if (live) setError(asGuiError(err).message); })
      .finally(() => { if (live) setBusy(false); });
    return () => { live = false; };
  }, [target.project_id, target.feature_id]);

  // Background work (a comment fetch, a list read or an investigation) reports through polls.
  const waiting = view?.stage === "loading" || !!view?.picker?.loading || view?.review?.investigating != null;
  useEffect(() => {
    if (!view || !waiting || busy) return;
    const workflowId = view.workflow_id;
    const timer = window.setInterval(() => {
      void prTriageSnapshot(workflowId).then((next) => {
        setView((current) => current?.workflow_id === workflowId && !pending.current ? next : current);
      }).catch((err) => setError(asGuiError(err).message));
    }, 700);
    return () => window.clearInterval(timer);
  }, [view?.workflow_id, waiting, busy]);

  // Seed the local reply editor once per opened reply; later polls never
  // overwrite what the user typed.
  useEffect(() => {
    const open = view?.reply;
    if (!open) { setReply(null); return; }
    const key = `${open.comment_id}:${open.kind}`;
    setReply((current) => current?.key === key ? current : { key, text: open.seed, seed: open.seed });
  }, [view?.reply?.comment_id, view?.reply?.kind]);

  const act = useCallback(async (action: PrTriageAction): Promise<boolean> => {
    if (!view || pending.current) return false;
    pending.current = true;
    setBusy(true);
    try {
      const next = await prTriageAct(view, action);
      setError(null);
      if (next === null) onClose(); else setView(next);
      return true;
    } catch (err) {
      setError(asGuiError(err).message);
      // A refusal may have changed what is pending; show the current state.
      try { setView(await prTriageSnapshot(view.workflow_id)); } catch { /* keep the last view */ }
      return false;
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }, [view, onClose]);

  const review = view?.review ?? null;
  const imageIdentity = view && review ? `${view.workflow_id}:${review.number}:${review.head_sha}:${review.fetched_at}` : "";
  const showImage = useCallback<OpenPrImage>((image, caption, trigger) => {
    imageTrigger.current = trigger;
    setOpenImage({ image, caption, identity: imageIdentity });
  }, [imageIdentity]);
  const closeImage = useCallback(() => {
    setOpenImage(null);
    window.requestAnimationFrame(() => imageTrigger.current?.focus());
  }, []);
  const currentImage = openImage?.identity === imageIdentity ? openImage : null;
  const comments = review?.comments ?? [];
  const selected = comments.find((c) => c.id === selectedId) ?? comments[0] ?? null;

  // An investigation draft belongs to one comment. Moving off an empty one
  // drops it; a typed one stays, announced on every other comment.
  useEffect(() => {
    setInvestigate((current) => current && current.commentId !== selected?.id && !current.text.trim() ? null : current);
  }, [selected?.id]);
  const replyDirty = reply !== null && reply.text !== reply.seed;
  const dirty = replyDirty || !!investigate?.text.trim();
  const locked = busy || !!view?.precall || !!view?.write_confirm || review?.investigating != null;
  const close = () => {
    if (dirty) setConfirmClose(true);
    else if (view) void act({ kind: "close" }); else onClose();
  };

  // The draft outlives the preview: it is cleared only once the call starts,
  // so a refused or failed confirmation never costs the user their text.
  function previewInvestigation() {
    if (!investigate) return;
    const text = investigate.text.trim();
    void act({
      kind: "investigate", comment_id: investigate.commentId, harness: investigate.harness,
      note: investigate.followUp ? null : text || null, follow_up: investigate.followUp ? text : null,
    });
  }

  async function confirmInvestigation() {
    if (await act({ kind: "precall_confirm" })) setInvestigate(null);
  }

  return (
    <>
    <Modal label="PR Triage" size="xl" dismissable={!busy && !currentImage} onClose={close}
      title={view ? `PR Triage · ${view.feature_name}` : "PR Triage"}
      subtitle="Read review feedback, investigate it read-only, and reply. GitHub writes and AI calls always ask first.">
      {confirmClose && <div className="callout callout-warning" role="alert">
        <p>Discard your unsent reply or investigation text and close PR Triage?</p>
        <button className="btn btn-secondary" onClick={() => setConfirmClose(false)}>Keep editing</button>
        <button className="btn btn-warning" disabled={busy} onClick={() => void act({ kind: "close" })}>Discard and close</button>
      </div>}
      {error && <div className="callout callout-danger" role="alert"><p>{error}</p></div>}
      {view?.error && view.error !== error && <div className="callout callout-danger" role="alert"><p>{view.error}</p></div>}
      {view?.notice && <p role="status" className="muted">{view.notice}</p>}
      {!view && busy && <p className="row"><Spinner /> Finding this branch's pull request…</p>}

      {view?.stage === "pick" && view.picker && <section aria-label="Choose a pull request" className="pr-picker">
        <div className="pr-toolbar">
          <label className="row small">
            <input type="checkbox" checked={view.picker.include_closed} disabled={busy}
              onChange={() => void act({ kind: "toggle_closed" })} /> Include closed and merged
          </label>
          <form className="row" onSubmit={(event) => {
            event.preventDefault();
            const number = Number(numberDraft);
            if (Number.isInteger(number) && number > 0) void act({ kind: "open", number });
          }}>
            <Field label="PR number">
              <input inputMode="numeric" value={numberDraft} disabled={busy} placeholder="e.g. 654"
                onChange={(e) => setNumberDraft(e.target.value.replace(/\D/g, ""))} />
            </Field>
            <button className="btn btn-secondary" disabled={busy || !numberDraft}>Open by number</button>
          </form>
        </div>
        {view.picker.error && <div className="callout callout-warning" role="alert"><p>Could not list pull requests: {view.picker.error}</p></div>}
        {view.picker.loading && <p className="row"><Spinner /> Loading pull requests…</p>}
        {view.picker.entries.length === 0 && !view.picker.error && !view.picker.loading && <p className="muted">No {view.picker.include_closed ? "" : "open "}pull requests. Open one by number instead.</p>}
        <ul className="pr-list">
          {view.picker.entries.map((entry) => <li key={entry.number}>
            <button className="pr-entry" disabled={busy} onClick={() => void act({ kind: "open", number: entry.number })}
              aria-label={`Open PR #${entry.number} ${entry.title}`}>
              <span className="row"><strong>#{entry.number}</strong> <span className="truncate">{entry.title}</span></span>
              <span className="row small muted">
                <span className="mono">{entry.head_ref}</span> · {entry.author}
                {entry.number === view.picker!.branch_pr && <span className="tag tag-accent">This branch</span>}
                {entry.mine && <span className="tag">Yours</span>}
                {entry.is_draft && <span className="tag">Draft</span>}
                {entry.state !== "OPEN" && <span className="tag">{entry.state.toLowerCase()}</span>}
              </span>
            </button>
          </li>)}
        </ul>
      </section>}

      {view?.stage === "loading" && <section className="pr-loading" aria-label="Loading comments">
        <p className="row"><Spinner /> Loading review comments for PR #{view.loading_pr}…</p>
        <button className="btn btn-secondary" disabled={busy} onClick={() => void act({ kind: "back_to_list" })}>Back to pull requests</button>
      </section>}

      {review && <section aria-label={`PR #${review.number} review comments`}>
        <div className="pr-toolbar">
          <div className="pr-heading">
            <strong>PR #{review.number}</strong> <span className="mono small">{review.head_ref} @ {review.head_sha.slice(0, 7)}</span>
            <span className="small muted"> · {review.open_count} open of {review.total} · fetched {review.fetched_at}</span>
          </div>
          <label className="row small">
            <input type="checkbox" checked={review.hide_resolved} disabled={locked}
              onChange={(e) => void act({ kind: "view", hide_resolved: e.target.checked, sort: review.sort })} /> Hide resolved
            {review.hidden_resolved > 0 && ` (${review.hidden_resolved} hidden)`}
          </label>
          <Field label="Sort">
            <select value={review.sort} disabled={locked} onChange={(e) => void act({ kind: "view", hide_resolved: review.hide_resolved, sort: e.target.value as PrSort })}>
              {SORTS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}
            </select>
          </Field>
          <button className="btn btn-secondary btn-sm" disabled={locked || reply !== null} onClick={() => void act({ kind: "refresh" })}>Refresh comments</button>
          <button className="btn btn-ghost btn-sm" disabled={locked || reply !== null} onClick={() => void act({ kind: "back_to_list" })}>Pull requests</button>
        </div>
        {view && <PrDescription key={imageIdentity} workflowId={view.workflow_id} identity={imageIdentity} onOpenImage={showImage} />}
        {review.branch_mismatch && <div className="callout callout-warning"><p>This checkout is on <span className="mono">{review.branch_mismatch}</span>, not the PR branch <span className="mono">{review.head_ref}</span>.</p></div>}

        {view?.precall && <section className="review-confirm" role="alertdialog" aria-label="Investigation AI call">
          <p>Headless AI call: {view.precall.title} · {view.precall.harness}. It reads the checkout without changing it and may use paid harness credits.</p>
          {view.precall.viewing && <pre className="doc doc-mono">{view.precall.preview}</pre>}
          <button className="btn btn-secondary" disabled={busy} onClick={() => void act({ kind: "precall_toggle_view" })}>{view.precall.viewing ? "Hide prompt" : "View prompt"}</button>
          <button className="btn btn-primary" disabled={busy} onClick={() => void confirmInvestigation()}>Continue AI call</button>
          <button className="btn btn-secondary" disabled={busy} onClick={() => void act({ kind: "precall_cancel" })}>Cancel AI call</button>
        </section>}

        {view?.write_confirm && <section className="review-confirm" role="alertdialog" aria-label="Confirm GitHub write">
          <p><strong>{view.write_confirm.destination}</strong>. This writes to GitHub as your <span className="mono">gh</span> account.</p>
          {view.write_confirm.body !== null && <pre className="doc pr-posted">{view.write_confirm.body}</pre>}
          <button className="btn btn-primary" disabled={busy} onClick={() => void act({ kind: "confirm_write" })}>
            {view.write_confirm.kind === "reply" ? "Post reply to GitHub" : view.write_confirm.kind === "resolve" ? "Resolve thread on GitHub" : "Reopen thread on GitHub"}
          </button>
          <button className="btn btn-secondary" disabled={busy} onClick={() => void act({ kind: "cancel_write" })}>Cancel</button>
        </section>}

        {review.investigating != null && <div className="callout callout-accent" role="status">
          <p className="row"><Spinner /> Investigating comment read-only with {review.investigating_harness}…</p>
          <button className="btn btn-secondary btn-sm" disabled={busy} onClick={() => void act({ kind: "cancel_investigation" })}>Cancel investigation</button>
        </div>}

        <div className="pr-reader">
          <nav className="pr-comments" aria-label="Review comments">
            {comments.length === 0 && <p className="muted pad">No comments to show.</p>}
            {comments.map((comment, index) => <div key={comment.id}>
              {index === review.conversation_start && <div className="pr-section">Conversation</div>}
              <button className={`pr-comment${comment.id === selected?.id ? " pr-comment-selected" : ""}`}
                aria-pressed={comment.id === selected?.id} onClick={() => setSelectedId(comment.id)}>
                <span className="row small"><strong>{comment.author}</strong>{comment.is_bot && <span className="tag">bot</span>}
                  <span className="muted truncate">{location(comment)}</span></span>
                <span className="small truncate">{comment.snippet || comment.body.split("\n")[0]}</span>
                <span className="row">
                  {comment.resolved && <span className="tag">resolved</span>}
                  {comment.outdated && <span className="tag">outdated</span>}
                  {comment.triage !== "untriaged" && <span className="tag tag-accent">{comment.triage}</span>}
                  {comment.investigation && <span className="tag">investigation {comment.investigation.status}</span>}
                  {comment.replies.length > 0 && <span className="tag">{comment.replies.length} repl{comment.replies.length === 1 ? "y" : "ies"}</span>}
                </span>
              </button>
            </div>)}
          </nav>
          {selected && <article className="pr-detail" aria-label="Selected comment">
            <header className="row">
              <strong>{selected.author}</strong><span className="muted small">{location(selected)}</span>
              {selected.resolved && <span className="tag">resolved</span>}
              {selected.triage !== "untriaged" && <span className="tag tag-accent">{selected.triage}</span>}
            </header>
            {selected.hunk && <pre className="pr-hunk" aria-label="Diff context">
              {selected.hunk.split("\n").map((line, i) => <code key={i} className={hunkClass(line)}>{line}{"\n"}</code>)}
            </pre>}
            <div className="pr-body"><PrMarkdown source={selected.body} workflowId={view!.workflow_id} identity={imageIdentity} onOpenImage={showImage} /></div>
            {selected.actionable && !selected.local_finding && <div className="pr-actions">
              <button className="btn btn-secondary btn-sm" disabled={locked} onClick={() => void act({ kind: "toggle_done", comment_id: selected.id })}>
                {selected.triage === "done" ? "Clear done" : "Mark done (local)"}</button>
              <button className="btn btn-secondary btn-sm" disabled={locked} onClick={() => void act({ kind: "toggle_skipped", comment_id: selected.id })}>
                {selected.triage === "skipped" ? "Clear skip" : "Skip (local)"}</button>
              <button className="btn btn-secondary btn-sm" disabled={locked || investigate !== null}
                onClick={() => setInvestigate({ commentId: selected.id, harness: view!.default_harness ?? view!.harnesses[0], text: "", followUp: false })}>Investigate…</button>
              {selected.investigation?.answer && <button className="btn btn-secondary btn-sm" disabled={locked || investigate !== null}
                onClick={() => setInvestigate({ commentId: selected.id, harness: view!.default_harness ?? view!.harnesses[0], text: "", followUp: true })}>Ask a follow-up…</button>}
              {selected.investigation && selected.investigation.status !== "dismissed" && selected.investigation.status !== "running" &&
                <button className="btn btn-ghost btn-sm" disabled={locked} onClick={() => void act({ kind: "dismiss_investigation", comment_id: selected.id })}>Dismiss investigation</button>}
              {(["done", "not_needed", "investigation"] as PrReplyKind[])
                .filter((kind) => kind !== "investigation" || !!selected.investigation?.answer)
                .map((kind) => <button key={kind} className="btn btn-secondary btn-sm" disabled={locked || view?.reply != null}
                  onClick={() => void act({ kind: "start_reply", comment_id: selected.id, reply: kind })}>{REPLY_LABELS[kind]}</button>)}
              {selected.can_resolve && <button className="btn btn-secondary btn-sm" disabled={locked}
                onClick={() => void act({ kind: "request_resolve", comment_id: selected.id })}>{selected.resolved ? "Reopen thread…" : "Resolve thread…"}</button>}
            </div>}
            {!selected.actionable && <p className="small muted">AMF follow-up replies are shown for context only.</p>}
            {investigate && investigate.commentId !== selected.id && <p className="small muted">
              An investigation request for another comment is open.{" "}
              {comments.some((c) => c.id === investigate.commentId) &&
                <><button className="link-button" onClick={() => setSelectedId(investigate.commentId)}>Go to it</button>{" · "}</>}
              <button className="link-button" disabled={busy} onClick={() => setInvestigate(null)}>Discard it</button>
            </p>}
            {view?.reply && view.reply.comment_id !== selected.id && <p className="small muted">
              A reply to another comment is open.{" "}
              <button className="link-button" onClick={() => setSelectedId(view.reply!.comment_id)}>Go to the reply</button>
            </p>}

            {investigate && investigate.commentId === selected.id && <section className="review-editor" aria-label="Investigation request" ref={reveal}>
              <Field label="Investigating harness">
                <select value={investigate.harness} disabled={busy || !!view?.precall} onChange={(e) => setInvestigate({ ...investigate, harness: e.target.value as AgentSlug })}>
                  {view!.harnesses.map((h) => <option key={h} value={h}>{h}</option>)}
                </select>
              </Field>
              <Field label={investigate.followUp ? "Follow-up question" : "What do you suspect? (optional)"}>
                <textarea rows={3} value={investigate.text} maxLength={1000} disabled={!!view?.precall}
                  onChange={(e) => setInvestigate({ ...investigate, text: e.target.value })} />
              </Field>
              <button className="btn btn-primary" disabled={locked || (investigate.followUp && !investigate.text.trim())}
                onClick={previewInvestigation}>Preview AI call</button>
              <button className="btn btn-ghost" disabled={busy || !!view?.precall} onClick={() => setInvestigate(null)}>Cancel</button>
            </section>}

            {view?.reply && view.reply.comment_id === selected.id && reply && <section className="review-editor" aria-label="Reply draft" ref={reveal}>
              <Field label={REPLY_LABELS[view.reply.kind]} hint={view.reply.agent_drafted ? "Drafted by the fixing agent; editing it changes its attribution." : "Nothing is posted until you confirm."}>
                <textarea rows={5} value={reply.text} onChange={(e) => setReply({ ...reply, text: e.target.value })} />
              </Field>
              <button className="btn btn-primary" disabled={locked || !reply.text.trim()}
                onClick={() => void act({ kind: "prepare_reply", comment_id: selected.id, body: reply.text })}>Review reply…</button>
              <button className="btn btn-ghost" disabled={locked} onClick={() => void act({ kind: "discard_reply" })}>Discard reply</button>
            </section>}
            {selected.local_note && <p className="small muted">Local note: {selected.local_note}</p>}
            {selected.replies.length > 0 && <section aria-label="Thread replies" className="pr-replies">
              {selected.replies.map((r) => <div key={r.id} className="pr-reply">
                <strong className="small">{r.author}</strong>{r.via_amf && <span className="tag">via AMF</span>}
                <PrMarkdown source={r.body} workflowId={view!.workflow_id} identity={imageIdentity} onOpenImage={showImage} />
              </div>)}
            </section>}

            {selected.investigation && <section className="review-note" aria-label="Investigation">
              <p className="row small"><strong>Investigation (read-only)</strong>
                <span className="tag">{selected.investigation.status}</span><span className="tag">{selected.investigation.harness}</span>
                {selected.investigation.stale_head && <span className="tag tag-red">older PR head</span>}</p>
              {selected.investigation.error && <p className="error-text">{selected.investigation.error}</p>}
              {selected.investigation.answer && <Markdown source={selected.investigation.answer} />}
              {selected.investigation.follow_ups.map((turn, i) => <div key={i} className="pr-follow-up">
                <p className="small"><strong>Follow-up {i + 1}:</strong> {turn.question}</p>
                <Markdown source={turn.answer} />
              </div>)}
            </section>}

          </article>}
        </div>
      </section>}
    </Modal>
    {currentImage && <Modal label="PR image" title={currentImage.caption} size="xl" onClose={closeImage}>
      <ScreenshotViewer identity={imageIdentity} caption={currentImage.caption} provenance={[]} index={0} total={1} backLabel="Close image"
        load={() => Promise.resolve(currentImage.image)} onMove={() => {}} onClose={closeImage} />
    </Modal>}
    </>
  );
}
