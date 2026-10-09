import { useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { asGuiError, type SessionTarget } from "./api";
import Markdown from "./Markdown";
import FreshContextDialog from "./FreshContextDialog";
import {
  sessionSidebar, sessionSidebarCompleteTodo, sessionSidebarPlan,
  type SessionPlanView, type SessionSidebarSection, type SidebarAction, type SidebarContextMeter,
  type SidebarLine,
} from "./sessionSidebarApi";
import { useSidebarCollapsed, useSidebarShortcut } from "./sidebarPrefs";
import { Icon, Modal, Spinner } from "./ui";
import "./sessionSidebar.css";

const POLL_MS = 2_000;
const ITEM_MARK = { done: "✓", active: "●", pending: "○" } as const;

/** The TUI's leader key for each action, named in its tooltip. */
const TUI_KEY: Record<SidebarAction["kind"], string> = {
  open_plan: "leader n",
  reuse_prompt: "leader l",
  pr_triage: "leader G",
  complete_todo: "leader z",
  supervised_edits: "leader V",
};

export const sessionSidebarKey = (target: SessionTarget) =>
  ["session-sidebar", target.project_id, target.feature_id, target.session_id];

/** The TUI's agent sidebar beside an agent tab: the same sections, in the
 * same order, each shown only when it has something to say. Collapsing it
 * is a per-viewer preference; the terminal beside it refits (and resizes
 * its tmux pane) through its own ResizeObserver. */
export default function SessionSidebar({
  target,
  onReusePrompt,
  onPrTriage,
  onSupervisedEdits,
  onFreshSession,
}: {
  target: SessionTarget;
  /** Append a prompt to this session's composer draft. */
  onReusePrompt: (prompt: string) => void;
  onPrTriage: () => void;
  onSupervisedEdits: () => void;
  onFreshSession: (target: SessionTarget, draft: string) => void;
}) {
  const [collapsed, setCollapsed] = useSidebarCollapsed("sessionSidebar");
  useSidebarShortcut("sessionSidebar", () => setCollapsed(!collapsed));
  const sidebar = useQuery({
    queryKey: sessionSidebarKey(target),
    queryFn: () => sessionSidebar(target),
    enabled: !collapsed,
    refetchInterval: collapsed ? false : POLL_MS,
  });
  const [plan, setPlan] = useState<{ loading: boolean; view?: SessionPlanView; error?: string } | null>(null);
  const planRequest = useRef(0);
  const [freshContext, setFreshContext] = useState(false);

  if (collapsed) {
    return (
      <aside className="agent-sidebar agent-sidebar-collapsed" aria-label="Agent sidebar">
        <button
          type="button"
          className="agent-sidebar-rail"
          aria-label="Show agent sidebar"
          aria-expanded={false}
          title="Show agent sidebar (Alt+Shift+A outside terminal and text inputs)"
          onClick={() => setCollapsed(false)}
        >
          <span className="agent-sidebar-flip"><Icon name="chevronRight" size={14} /></span>
          <span className="agent-sidebar-rail-label">Sidebar</span>
        </button>
      </aside>
    );
  }

  const view = sidebar.error ? undefined : sidebar.data;
  const error = sidebar.error ? asGuiError(sidebar.error).message : null;

  const openPlan = () => {
    const request = ++planRequest.current;
    setPlan({ loading: true });
    sessionSidebarPlan(target).then(
      (result) => { if (request === planRequest.current) setPlan({ loading: false, view: result }); },
      (err) => { if (request === planRequest.current) setPlan({ loading: false, error: asGuiError(err).message }); },
    );
  };

  return (
    <aside className="agent-sidebar" data-harness={view?.harness} aria-label={view?.title ?? "Agent sidebar"}>
      <header className="agent-sidebar-head">
        <h2>{view?.title ?? "Agent sidebar"}</h2>
        <button
          type="button"
          className="btn btn-ghost btn-icon btn-sm"
          aria-label="Hide agent sidebar"
          aria-expanded
          title="Hide agent sidebar (Alt+Shift+A outside terminal and text inputs)"
          onClick={() => setCollapsed(true)}
        >
          <Icon name="chevronRight" size={14} />
        </button>
      </header>
      <div className="agent-sidebar-body">
        {error && <p role="alert" className="agent-sidebar-error">{error}</p>}
        {!view && !error && <p className="agent-sidebar-loading"><Spinner /> Loading…</p>}
        {view?.sections.map((section) => (
          <Section
            key={section.kind}
            target={target}
            section={section}
            onOpenPlan={openPlan}
            onFreshContext={() => setFreshContext(true)}
            onReusePrompt={onReusePrompt}
            onPrTriage={onPrTriage}
            onSupervisedEdits={onSupervisedEdits}
          />
        ))}
        {view?.notes.map((note) => <p key={note} className="agent-sidebar-note">{note}</p>)}
      </div>
      {freshContext && <FreshContextDialog target={target} onClose={() => setFreshContext(false)}
        onCreated={onFreshSession} />}
      {plan && (
        <Modal
          label="Current plan"
          title="Current plan"
          subtitle={plan.view && <span className="mono">{plan.view.path}</span>}
          size="lg"
          onClose={() => { ++planRequest.current; setPlan(null); }}
        >
          {plan.loading && <p><Spinner /> Reading the plan…</p>}
          {plan.error && <p role="alert" className="agent-sidebar-error">{plan.error}</p>}
          {plan.view && (
            <>
              <Markdown source={plan.view.markdown} />
              {plan.view.truncated && (
                <p className="agent-sidebar-note">The plan is longer than the GUI shows; open the file to read the rest.</p>
              )}
            </>
          )}
        </Modal>
      )}
    </aside>
  );
}

function Section({
  target,
  section,
  onOpenPlan,
  onFreshContext,
  onReusePrompt,
  onPrTriage,
  onSupervisedEdits,
}: {
  target: SessionTarget;
  section: SessionSidebarSection;
  onOpenPlan: () => void;
  onFreshContext: () => void;
  onReusePrompt: (prompt: string) => void;
  onPrTriage: () => void;
  onSupervisedEdits: () => void;
}) {
  const queryClient = useQueryClient();
  const [showFull, setShowFull] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [outcome, setOutcome] = useState<{ tone: "ok" | "error"; text: string } | null>(null);
  const complete = useMutation({
    mutationFn: (todoId: string) => sessionSidebarCompleteTodo(target, todoId),
    onSuccess: (message) => {
      setConfirming(false);
      setOutcome({ tone: "ok", text: message });
      void queryClient.invalidateQueries({ queryKey: sessionSidebarKey(target) });
      void queryClient.invalidateQueries({ queryKey: ["todos"] });
    },
    onError: (err) => {
      setConfirming(false);
      setOutcome({ tone: "error", text: asGuiError(err).message });
    },
  });
  const fullPrompt = section.actions.find((action) => action.kind === "reuse_prompt");
  const completeTodo = section.actions.find((action) => action.kind === "complete_todo");

  const button = (action: SidebarAction) => {
    const title = (text: string) => `${text} (TUI: ${TUI_KEY[action.kind]})`;
    switch (action.kind) {
      case "open_plan":
        return <button key={action.kind} type="button" className="sb-action" title={title("Read the current plan")} onClick={onOpenPlan}>Open</button>;
      case "reuse_prompt":
        return (
          <span key={action.kind} className="sb-action-group">
            <button type="button" className="sb-action" aria-expanded={showFull} onClick={() => setShowFull((shown) => !shown)}>
              {showFull ? "Less" : "View"}
            </button>
            <button type="button" className="sb-action" title={title("Add this prompt to the composer draft")}
              onClick={() => onReusePrompt(action.prompt)}>Reuse</button>
          </span>
        );
      case "pr_triage":
        return <button key={action.kind} type="button" className="sb-action" title={title("Open PR Triage")} onClick={onPrTriage}>Triage</button>;
      case "supervised_edits":
        return <button key={action.kind} type="button" className="sb-action" title={title("Answer the pending review")} onClick={onSupervisedEdits}>Review</button>;
      case "complete_todo":
        return (
          <button key={action.kind} type="button" className="sb-action" title={title("Mark the linked TODO complete")}
            disabled={complete.isPending || confirming} onClick={() => { setOutcome(null); setConfirming(true); }}>
            Complete
          </button>
        );
    }
  };

  return (
    <section
      className={`sb-section sb-${section.kind}`}
      data-band={section.context?.band}
      aria-label={section.title}
    >
      <header className="sb-section-head">
        <h3>{section.title}</h3>
        {section.actions.length > 0 && <div className="sb-actions">{section.actions.map(button)}</div>}
      </header>
      {section.context && <ContextMeter meter={section.context} onFreshContext={onFreshContext} />}
      <div className="sb-lines">
        {section.lines.map((line, index) => <Line key={index} line={line} />)}
      </div>
      {fullPrompt?.kind === "reuse_prompt" && showFull && (
        <pre className="sb-prompt-full" aria-label="Full prompt">{fullPrompt.prompt}</pre>
      )}
      {completeTodo?.kind === "complete_todo" && confirming && (
        <div className="sb-confirm" role="group" aria-label="Confirm TODO completion">
          <span>Mark this TODO complete?</span>
          <button type="button" className="btn btn-primary btn-sm" disabled={complete.isPending}
            onClick={() => complete.mutate(completeTodo.todo_id)}>
            {complete.isPending && <Spinner />} Complete
          </button>
          <button type="button" className="btn btn-ghost btn-sm" disabled={complete.isPending}
            onClick={() => setConfirming(false)}>Cancel</button>
        </div>
      )}
      {outcome && (
        <p role={outcome.tone === "error" ? "alert" : "status"} className={`sb-outcome sb-outcome-${outcome.tone}`}>
          {outcome.text}
        </p>
      )}
    </section>
  );
}

function ContextMeter({ meter, onFreshContext }: { meter: SidebarContextMeter; onFreshContext: () => void }) {
  const percent = Math.max(0, Math.min(100, meter.percent));
  return (
    <div className="sb-context-meter">
      <div
        className="sb-track"
        role="meter"
        aria-label="Context window used"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >
        <span className={`sb-fill ctx-fill-${meter.band}`} style={{ width: `${percent}%` }} />
      </div>
      <p className="sb-context-detail">
        {meter.used_tokens.toLocaleString("en-US")} of {meter.limit_tokens.toLocaleString("en-US")} tokens
        {meter.estimated && <span className="sb-chip">estimated</span>}
        {meter.stale && <span className="sb-chip sb-chip-stale">stale</span>}
      </p>
      <button className="sb-action" onClick={onFreshContext}>Fresh context</button>
      {meter.fresh_context_hint && (
        <p className="sb-hint">Context is filling up.</p>
      )}
    </div>
  );
}

function Line({ line }: { line: SidebarLine }) {
  switch (line.kind) {
    case "field":
      return (
        <p className="sb-line">
          <span className="sb-label">{line.label}</span>
          <span className={`sb-value tone-${line.tone}${line.emphasised ? " sb-strong" : ""}`}>{line.value}</span>
        </p>
      );
    case "text":
      return <p className={`sb-line sb-text tone-${line.tone}${line.emphasised ? " sb-strong" : ""}`}>{line.text}</p>;
    case "bar": {
      const used = Math.max(0, Math.min(100, line.used_percent));
      return (
        <div className="sb-line sb-bar">
          <span className="sb-bar-label">{line.label}</span>
          <span className="sb-track" role="meter" aria-label={`${line.label} usage`}
            aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(used)}>
            <span className={`sb-fill usage-${line.level}`} style={{ width: `${used}%` }} />
          </span>
          <span className={`sb-bar-pct usage-text-${line.level}`}>{Math.round(used)}%</span>
          {line.reset && <span className="sb-bar-reset" title="Time to reset">{line.reset}</span>}
        </div>
      );
    }
    case "progress": {
      const done = line.total > 0 ? (line.done / line.total) * 100 : 0;
      return (
        <div className="sb-line sb-bar">
          <span className="sb-track" role="progressbar" aria-label="Todos done"
            aria-valuemin={0} aria-valuemax={line.total} aria-valuenow={line.done}>
            <span className="sb-fill sb-fill-todo" style={{ width: `${done}%` }} />
          </span>
          <span className="sb-bar-pct">{line.done}/{line.total}</span>
        </div>
      );
    }
    case "item":
      return (
        <p className={`sb-line sb-item sb-item-${line.state}`}>
          <span className="sb-item-mark" aria-label={line.state}>{ITEM_MARK[line.state]}</span>
          <span>{line.text}</span>
        </p>
      );
    case "more":
      return <p className="sb-line sb-more">{line.text}</p>;
  }
}
