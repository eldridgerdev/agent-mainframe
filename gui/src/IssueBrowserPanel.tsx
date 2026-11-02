import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AgentSlug, HarnessInfo, ModeInfo, ModeSlug, asGuiError } from "./api";
import {
  IssueFixResponse, IssueListView, IssuePreview, issueBrowserList, issueFixCreate, issueFixPreview,
} from "./issueApi";
import { ApprovalDialog, Field, Icon, Modal, Spinner, Switch } from "./ui";
import WorktreeHookField, { useWorktreeHookChoice } from "./WorktreeHookField";

/** The issue text is GitHub-authored data: it is shown as plain text, never
 * rendered as markup, and reaches the agent only inside the labeled prompt. */
export function matchesIssue(
  row: { number: number; title: string; labels: string[]; excerpt: string },
  query: string,
  label: string,
): boolean {
  if (label && !row.labels.includes(label)) return false;
  const needle = query.trim().toLowerCase().replace(/^#/, "");
  if (!needle) return true;
  return String(row.number) === needle
    || row.title.toLowerCase().includes(needle)
    || row.excerpt.toLowerCase().includes(needle)
    || row.labels.some((name) => name.toLowerCase().includes(needle));
}

export default function IssueBrowserPanel({
  projectId, projectName, agents, modes, isGit, onClose, onCreated,
}: {
  projectId: string;
  projectName: string;
  agents: HarnessInfo[];
  modes: ModeInfo[];
  isGit: boolean;
  onClose: () => void;
  onCreated: (response: IssueFixResponse) => void;
}) {
  const [page, setPage] = useState(1);
  const [list, setList] = useState<IssueListView | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [label, setLabel] = useState("");
  const [preview, setPreview] = useState<IssuePreview | null>(null);
  const [opening, setOpening] = useState<number | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);
  const requestId = useRef(0);

  const load = useCallback(async (target: number) => {
    const id = ++requestId.current;
    setLoading(true);
    setListError(null);
    try {
      const view = await issueBrowserList(projectId, target);
      if (id !== requestId.current) return;
      setList(view);
      setPage(view.page);
    } catch (err) {
      if (id !== requestId.current) return;
      setListError(asGuiError(err).message);
    } finally {
      if (id === requestId.current) setLoading(false);
    }
  }, [projectId]);

  useEffect(() => { void load(1); }, [load]);

  const labels = useMemo(
    () => [...new Set((list?.issues ?? []).flatMap((row) => row.labels))].sort(),
    [list],
  );
  const shown = (list?.issues ?? []).filter((row) => matchesIssue(row, query, label));

  async function open(number: number) {
    setOpening(number);
    setOpenError(null);
    try {
      setPreview(await issueFixPreview(projectId, number));
    } catch (err) {
      setOpenError(asGuiError(err).message);
    } finally {
      setOpening(null);
    }
  }

  if (preview) {
    return (
      <IssueFixForm
        key={`${preview.number}:${preview.digest}`}
        preview={preview}
        projectId={projectId}
        projectName={projectName}
        agents={agents}
        modes={modes}
        isGit={isGit}
        onBack={() => setPreview(null)}
        onReload={async () => setPreview(await issueFixPreview(projectId, preview.number))}
        onClose={onClose}
        onCreated={onCreated}
      />
    );
  }

  return (
    <Modal
      label="GitHub issues"
      title="GitHub issues"
      subtitle={list ? <span className="mono">{list.repository}</span> : `in ${projectName}`}
      size="lg"
      onClose={onClose}
      footer={
        <>
          <span className="field-hint issue-footnote">Read-only: AMF never writes to GitHub from here.</span>
          <button className="btn btn-ghost" disabled={loading || page <= 1} onClick={() => void load(page - 1)}>
            Previous
          </button>
          <span className="count-pill">Page {page}</span>
          <button className="btn btn-ghost" disabled={loading || !list?.has_next_page} onClick={() => void load(page + 1)}>
            Next
          </button>
          <button className="btn btn-ghost" onClick={() => void load(page)} disabled={loading}>Refresh</button>
        </>
      }
    >
      <div className="form-stack">
        <div className="form-row">
          <Field label="Search this page">
            <input
              type="search"
              aria-label="Search issues"
              value={query}
              placeholder="Title, #number, label or text"
              onChange={(event) => setQuery(event.target.value)}
            />
          </Field>
          <Field label="Label">
            <select aria-label="Filter by label" value={label} onChange={(event) => setLabel(event.target.value)}>
              <option value="">All labels</option>
              {labels.map((name) => <option key={name} value={name}>{name}</option>)}
            </select>
          </Field>
        </div>
        {openError && <div className="callout callout-warning" role="alert"><p>{openError}</p></div>}
        {loading && <p className="field-hint"><Spinner /> Loading open issues…</p>}
        {!loading && listError && (
          <div className="callout callout-warning" role="alert">
            <p>{listError}</p>
            <button className="btn btn-secondary" onClick={() => void load(page)}>Retry</button>
          </div>
        )}
        {!loading && !listError && list && shown.length === 0 && (
          <p className="field-hint">
            {list.issues.length === 0
              ? "No open issues on this page."
              : "No issue on this page matches the search or label."}
          </p>
        )}
        {!listError && shown.length > 0 && (
          <ul className="issue-list" aria-label="Open issues">
            {shown.map((row) => (
              <li key={row.number}>
                <button
                  type="button"
                  className="pr-entry issue-entry"
                  aria-label={`Open issue #${row.number} ${row.title}`}
                  disabled={opening !== null}
                  onClick={() => void open(row.number)}
                >
                  <span className="issue-entry-title">
                    <span className="mono">#{row.number}</span> {row.title}
                    {opening === row.number && <Spinner />}
                  </span>
                  <span className="issue-entry-meta">
                    {row.labels.map((name) => <span key={name} className="tag">{name}</span>)}
                    {row.linked_features.length > 0 && (
                      <span className="tag tag-accent">
                        Feature exists: {row.linked_features.map((f) => f.feature_name).join(", ")}
                      </span>
                    )}
                    <span className="field-hint">Updated {row.updated_at}</span>
                  </span>
                  {row.excerpt && <span className="field-hint issue-excerpt">{row.excerpt}</span>}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </Modal>
  );
}

function IssueFixForm({
  preview, projectId, projectName, agents, modes, isGit, onBack, onReload, onClose, onCreated,
}: {
  preview: IssuePreview;
  projectId: string;
  projectName: string;
  agents: HarnessInfo[];
  modes: ModeInfo[];
  isGit: boolean;
  onBack: () => void;
  onReload: () => Promise<void>;
  onClose: () => void;
  onCreated: (response: IssueFixResponse) => void;
}) {
  const [featureName, setFeatureName] = useState(preview.suggested_feature_name);
  const [branch, setBranch] = useState(preview.suggested_branch);
  const [agent, setAgent] = useState<AgentSlug>(agents[0]?.slug ?? "claude");
  const [mode, setMode] = useState<ModeSlug>(modes.find((m) => m.slug === "vibeless")?.slug ?? modes[0]?.slug ?? "vibeless");
  const [useWorktree, setUseWorktree] = useState(isGit);
  const [prompt, setPrompt] = useState(preview.prompt);
  const [start, setStart] = useState(false);
  const [duplicate, setDuplicate] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [stale, setStale] = useState(false);
  const [approval, setApproval] = useState<string | null>(null);
  const hook = useWorktreeHookChoice(projectId, isGit && useWorktree);
  const hasDuplicate = preview.linked_features.length > 0;
  const ready = !busy && hook.ready && !!featureName.trim() && !!branch.trim() && !!prompt.trim()
    && (!hasDuplicate || duplicate);

  async function submit(approved: boolean) {
    if (!ready && !approved) return;
    setBusy(true);
    setError(null);
    try {
      const response = await issueFixCreate({
        project_id: projectId,
        number: preview.number,
        digest: preview.digest,
        feature_name: featureName.trim(),
        request: {
          project_name: projectName,
          branch: branch.trim(),
          agent,
          mode,
          review: false,
          plan_mode: false,
          create_terminal: false,
          use_worktree: useWorktree,
          enable_chrome: false,
          hook_choice: hook.choice || null,
          dry_run: false,
        },
        prompt,
        start,
        approved,
        allow_duplicate: duplicate,
      });
      setApproval(null);
      onCreated(response);
    } catch (err) {
      const failure = asGuiError(err);
      if (failure.kind === "needs_approval" && !approved) {
        setApproval(failure.message);
      } else {
        setApproval(null);
        setError(failure.message);
        setStale(failure.message.includes("changed on GitHub"));
      }
    } finally {
      setBusy(false);
    }
  }

  const modeInfo = modes.find((candidate) => candidate.slug === mode);
  return (
    <>
      <Modal
        label="Fix GitHub issue"
        title={<>Fix issue <span className="mono">#{preview.number}</span></>}
        subtitle={<span className="mono">{preview.repository}</span>}
        size="lg"
        onClose={onClose}
        dismissable={!busy}
        onSubmit={() => void submit(false)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={onBack} disabled={busy}>Back to issues</button>
            <button type="submit" className="btn btn-primary" disabled={!ready}>
              {busy && <Spinner />}
              {busy ? "Creating…" : start ? "Create and start" : "Create feature"}
            </button>
          </>
        }
      >
        <div className="form-stack">
          <section className="issue-preview" aria-label="Issue preview">
            <h3>{preview.title}</h3>
            <p className="issue-entry-meta">
              {preview.labels.map((name) => <span key={name} className="tag">{name}</span>)}
              <span className="field-hint">Updated {preview.updated_at}</span>
            </p>
            <pre className="issue-body">{preview.body || "(no description)"}</pre>
            {preview.body_truncated && <p className="field-hint">The description is shown truncated.</p>}
          </section>
          {hasDuplicate && (
            <div className="callout callout-warning" role="alert">
              <p>
                This issue already has a feature: {preview.linked_features.map((f) => f.feature_name).join(", ")}.
              </p>
              <Switch checked={duplicate} onChange={setDuplicate} label="Create another feature for this issue" disabled={busy} />
            </div>
          )}
          {error && (
            <div className="callout callout-warning" role="alert">
              <p>{error}</p>
              {stale && (
                <button type="button" className="btn btn-secondary" disabled={busy}
                  onClick={() => void onReload().catch((err) => setError(asGuiError(err).message))}>
                  Reload issue
                </button>
              )}
            </div>
          )}
          <Field label="Feature name">
            <input className="mono" value={featureName} onChange={(event) => setFeatureName(event.target.value)} disabled={busy} />
          </Field>
          <Field label="Branch">
            <input className="mono" value={branch} onChange={(event) => setBranch(event.target.value)} disabled={busy} />
          </Field>
          <div className="form-row">
            <Field label="Harness">
              <select value={agent} onChange={(event) => setAgent(event.target.value as AgentSlug)} disabled={busy}>
                {agents.map((a) => <option key={a.slug} value={a.slug}>{a.display_name}</option>)}
              </select>
            </Field>
            <Field label="Mode">
              <select value={mode} onChange={(event) => setMode(event.target.value as ModeSlug)} disabled={busy}>
                {modes.map((m) => <option key={m.slug} value={m.slug} title={m.description}>{m.display_name}</option>)}
              </select>
            </Field>
          </div>
          {modeInfo?.description && <p className="field-hint mode-hint">{modeInfo.description}</p>}
          {isGit && (
            <Switch checked={useWorktree} onChange={setUseWorktree} disabled={busy}
              label="Use a git worktree" hint="Gives the feature its own checkout so agents don't collide." />
          )}
          <WorktreeHookField hook={hook} disabled={busy} />
          <Field label="Agent prompt" hint="Shown to the agent as an unsent draft. The issue text inside is data, not instructions.">
            <textarea className="mono issue-prompt" rows={9} value={prompt}
              onChange={(event) => setPrompt(event.target.value)} disabled={busy} aria-label="Agent prompt" />
          </Field>
          <Switch checked={start} onChange={setStart} disabled={busy}
            label="Start the agent now"
            hint="Off: the feature is created stopped and no agent runs. On: the agent starts, subject to the resource limits." />
          <p className="field-hint">
            <Icon name="check" size={12} /> Nothing is posted to GitHub. If the issue changes before you confirm, creation is refused.
          </p>
        </div>
      </Modal>
      {approval && (
        <ApprovalDialog
          label="Approve agent start"
          title="Start another agent?"
          message={approval}
          confirmLabel="Start anyway"
          busy={busy}
          onConfirm={() => void submit(true)}
          onCancel={() => setApproval(null)}
        />
      )}
    </>
  );
}
