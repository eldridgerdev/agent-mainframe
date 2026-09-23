import { ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  AgentSlug,
  CreateFeatureRequest,
  Feature,
  FeatureTarget,
  GuiError,
  HarnessInfo,
  ModeInfo,
  ModeSlug,
  PlanAction,
  PlanInput,
  PlanStatus,
  Project,
  SessionTarget,
  WorkspaceSnapshot,
  asGuiError,
  createFeature,
  createProject,
  getSnapshot,
  planAct,
  planBegin,
  planBeginCreation,
  planBeginTodoHost,
  planBeginTodoNew,
  planSnapshot,
  startFeature,
  stopFeature,
  supportedHarnesses,
  supportedModes,
  terminalSubmitPrompt,
  todoLaunchAgent,
  todoLaunchNewFeature,
} from "./api";
import TerminalPane from "./TerminalPane";
import TodoPanel, { TodoAgentTarget, TodoDestination } from "./TodoPanel";
import PlanPanel from "./PlanPanel";
import {
  ApprovalDialog,
  EmptyState,
  Field,
  Icon,
  IconName,
  Menu,
  Modal,
  Segmented,
  Spinner,
  StatusBadge,
  StatusDot,
  Switch,
  Toast,
  Toasts,
} from "./ui";

const SNAPSHOT_KEY = ["workspace-snapshot"];
const PLAN_KEY = ["plan-interview"];
const TODOS_TAB = "todos";

type View =
  | { kind: "todos" }
  | { kind: "project"; projectId: string }
  | { kind: "feature"; projectId: string; featureId: string };

interface Draft {
  key: string;
  text: string;
}

const ERROR_TITLE: Record<GuiError["kind"], string> = {
  not_found: "Not found",
  conflict: "Out of date",
  needs_approval: "Approval needed",
  internal: "Something went wrong",
};

const LOADING_PHASES = new Set([
  "ai_loading", "synthesis_loading", "directed_feedback_loading",
  "investigation_loading", "critique_loading", "done",
]);

// Feature list order (sidebar and project page): running features first, then idle, then stopped. The sort
// is stable, so each group keeps the store's own order.
const STATUS_RANK: Record<Feature["status"], number> = { active: 0, idle: 1, stopped: 2 };
const byStatus = (features: Feature[]) =>
  [...features].sort((a, b) => STATUS_RANK[a.status] - STATUS_RANK[b.status]);

const sessionKey = (target: { feature_id: string; session_id: string }) =>
  `${target.feature_id}:${target.session_id}`;

/// Workspace shell: sidebar navigation (global TODOs, projects, features),
/// a main view for the selection, and modals for creation, approvals and
/// the plan interview. Id resolution, idempotency, and terminal correctness
/// have their own Rust-side coverage; this file is the UI wiring.
export default function App() {
  const queryClient = useQueryClient();
  const [view, setView] = useState<View | null>(null);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [showCreateProject, setShowCreateProject] = useState(false);
  const [createFeatureFor, setCreateFeatureFor] = useState<string | null>(null);
  const [tabByFeature, setTabByFeature] = useState<Record<string, string>>({});
  const [draft, setDraft] = useState<Draft | null>(null);
  const [sendingPrompt, setSendingPrompt] = useState(false);
  const [planMinimized, setPlanMinimized] = useState(false);
  const [pendingPlanApproval, setPendingPlanApproval] = useState<string | null>(null);
  const [pendingTodoNew, setPendingTodoNew] = useState<{
    todoId: string;
    title: string;
    projectIds: string[];
    kind: "plan" | "launch";
  } | null>(null);
  const [pendingTodoNewApproval, setPendingTodoNewApproval] = useState<{
    todoId: string;
    request: CreateFeatureRequest;
    message: string;
  } | null>(null);
  const [todoNewBusy, setTodoNewBusy] = useState(false);
  const [planBusy, setPlanBusy] = useState(false);
  const planActionInFlight = useRef(false);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const toastId = useRef(0);
  const [pendingStart, setPendingStart] = useState<{
    target: FeatureTarget;
    message: string;
  } | null>(null);
  const [pendingTodoLaunch, setPendingTodoLaunch] = useState<{
    todoId: string;
    target: FeatureTarget;
    message: string;
  } | null>(null);

  const harnesses = useQuery({
    queryKey: ["supported-harnesses"],
    queryFn: supportedHarnesses,
  });
  const modes = useQuery({ queryKey: ["supported-modes"], queryFn: supportedModes });
  const workspace = useQuery({
    queryKey: SNAPSHOT_KEY,
    queryFn: getSnapshot,
    refetchInterval: 2_000,
  });
  const plan = useQuery({
    queryKey: PLAN_KEY,
    queryFn: planSnapshot,
    refetchInterval: planBusy ? false : 750,
  });

  // "Resynchronization": every mutating command emits a fresh snapshot on
  // `workspace-changed` after it applies -- update the cache directly from
  // the event's payload instead of triggering a second round trip, and pick
  // this up even for changes this window did not cause itself (such as a
  // second window). Separate TUI processes cannot emit a Tauri event here;
  // the periodic get_snapshot above reads their commits.
  useEffect(() => {
    const unlistenPromise = listen<WorkspaceSnapshot>("workspace-changed", (event) => {
      queryClient.setQueryData(SNAPSHOT_KEY, event.payload);
    });
    return () => void unlistenPromise.then((unlisten) => unlisten());
  }, [queryClient]);

  const projects = workspace.data?.projects ?? [];
  const selectedProject: Project | undefined = view && view.kind !== "todos"
    ? projects.find((project) => project.id === view.projectId)
    : undefined;
  const selectedFeature: Feature | undefined = view?.kind === "feature"
    ? selectedProject?.features.find((feature) => feature.id === view.featureId)
    : undefined;

  // Land on the first project, and fall back gracefully when the selected
  // project or feature disappears (deleted from the TUI, for instance).
  useEffect(() => {
    if (!workspace.data) return;
    if (!view || (view.kind !== "todos" && !selectedProject)) {
      setView(projects[0] ? { kind: "project", projectId: projects[0].id } : null);
    } else if (view.kind === "feature" && !selectedFeature) {
      setView({ kind: "project", projectId: view.projectId });
    }
  }, [workspace.data, view, selectedProject, selectedFeature, projects]);

  const dismissToast = useCallback((id: number) => {
    setToasts((current) => current.filter((toast) => toast.id !== id));
  }, []);

  const pushToast = useCallback((toast: Omit<Toast, "id">) => {
    const id = ++toastId.current;
    setToasts((current) => [...current.slice(-3), { ...toast, id }]);
    window.setTimeout(() => dismissToast(id), toast.tone === "error" ? 12_000 : 6_000);
  }, [dismissToast]);

  const reportError = useCallback((err: unknown) => {
    const error = asGuiError(err);
    pushToast({
      tone: "error",
      title: ERROR_TITLE[error.kind] ?? "Error",
      message: error.message,
      action: error.kind === "conflict"
        ? { label: "Refresh", onClick: () => void workspace.refetch() }
        : undefined,
    });
  }, [pushToast, workspace]);

  const harnessName = (slug: AgentSlug) =>
    harnesses.data?.find((harness) => harness.slug === slug)?.display_name ?? slug;
  const modeName = (slug: ModeSlug) =>
    modes.data?.find((mode) => mode.slug === slug)?.display_name ?? slug;

  const todoDestinations: TodoDestination[] = [
    { label: "Global", scope: { kind: "global" } },
    ...projects.flatMap((project): TodoDestination[] => [
      { label: `Project: ${project.name}`, scope: { kind: "project", project_id: project.id } },
      ...project.features.map((feature): TodoDestination => ({
        label: `Feature: ${project.name} / ${feature.name}`,
        scope: { kind: "worktree", project_id: project.id, feature_id: feature.id },
      })),
    ]),
  ];
  const todoAgentTargets: TodoAgentTarget[] = projects.flatMap(
    (project) => project.features.map((feature) => ({
      label: `${project.name} / ${feature.name}`,
      target: { project_id: project.id, feature_id: feature.id },
    })),
  );
  const gitProjectIds = projects.filter((project) => project.is_git).map((project) => project.id);

  function newFeatureHandlers(projectIds: string[]) {
    if (projectIds.length === 0) return {};
    return {
      onPlanNew: (todoId: string, title: string) =>
        setPendingTodoNew({ todoId, title, projectIds, kind: "plan" }),
      onLaunchNew: (todoId: string, title: string) =>
        setPendingTodoNew({ todoId, title, projectIds, kind: "launch" }),
    };
  }

  /** Show a session's terminal, optionally with an editable, unsent prompt. */
  function openSession(target: SessionTarget, draftPrompt?: string) {
    setView({ kind: "feature", projectId: target.project_id, featureId: target.feature_id });
    setTabByFeature((current) => ({ ...current, [target.feature_id]: target.session_id }));
    if (draftPrompt !== undefined) setDraft({ key: sessionKey(target), text: draftPrompt });
  }

  function updatePlan(status: PlanStatus) {
    queryClient.setQueryData(PLAN_KEY, status);
    if (status.handoff) {
      openSession(status.handoff.target, status.handoff.draft_prompt);
      void queryClient.invalidateQueries({ queryKey: ["todos"] });
    }
    if (!status.active && status.message) {
      pushToast({ tone: "info", title: "Plan", message: status.message });
    }
  }

  async function runPlanBegin(start: () => Promise<PlanStatus>): Promise<boolean> {
    if (planActionInFlight.current) return false;
    planActionInFlight.current = true;
    setPlanBusy(true);
    try {
      await queryClient.cancelQueries({ queryKey: PLAN_KEY });
      updatePlan(await start());
      setPlanMinimized(false);
      return true;
    } catch (err) {
      reportError(err);
      return false;
    } finally {
      planActionInFlight.current = false;
      setPlanBusy(false);
    }
  }

  async function beginPlan(target: FeatureTarget, quick: boolean) {
    await runPlanBegin(() => planBegin(target, quick));
  }

  async function beginTodoPlan(todoId: string, target: FeatureTarget) {
    await runPlanBegin(() => planBeginTodoHost(todoId, target));
  }

  async function actOnPlan(action: PlanAction, input?: PlanInput) {
    if (planActionInFlight.current) return;
    const step = plan.data?.active?.step_key;
    if (!step) return;
    planActionInFlight.current = true;
    setPlanBusy(true);
    try {
      await queryClient.cancelQueries({ queryKey: PLAN_KEY });
      updatePlan(await planAct(step, action, input ?? null));
      setPendingPlanApproval(null);
    } catch (err) {
      const error = asGuiError(err);
      if (error.kind === "needs_approval" && action === "accept") {
        setPendingPlanApproval(error.message);
      } else {
        setPendingPlanApproval(null);
        reportError(error);
        void plan.refetch();
      }
    } finally {
      planActionInFlight.current = false;
      setPlanBusy(false);
    }
  }

  async function launchTodoAgent(todoId: string, target: FeatureTarget, approved: boolean) {
    try {
      const launched = await todoLaunchAgent(todoId, target, approved);
      openSession(launched.target, launched.draft_prompt);
      setPendingTodoLaunch(null);
      void queryClient.invalidateQueries({ queryKey: ["todos"] });
    } catch (err) {
      const error = asGuiError(err);
      if (error.kind === "needs_approval" && !approved) {
        setPendingTodoLaunch({ todoId, target, message: error.message });
      } else {
        setPendingTodoLaunch(null);
        reportError(error);
      }
    }
  }

  async function launchTodoNewFeature(
    todoId: string,
    request: CreateFeatureRequest,
    approved: boolean,
  ) {
    if (todoNewBusy) return;
    setTodoNewBusy(true);
    try {
      const launched = await todoLaunchNewFeature(todoId, request, approved);
      await workspace.refetch();
      openSession(launched.target, launched.draft_prompt);
      setPendingTodoNew(null);
      setPendingTodoNewApproval(null);
      void queryClient.invalidateQueries({ queryKey: ["todos"] });
    } catch (err) {
      const error = asGuiError(err);
      if (error.kind === "needs_approval" && !approved) {
        setPendingTodoNewApproval({ todoId, request, message: error.message });
      } else {
        setPendingTodoNewApproval(null);
        reportError(error);
        void workspace.refetch();
      }
    } finally {
      setTodoNewBusy(false);
    }
  }

  const createProjectMutation = useMutation({
    mutationFn: createProject,
    onSuccess: (response) => {
      setShowCreateProject(false);
      if (response.project_id) setView({ kind: "project", projectId: response.project_id });
    },
    onError: reportError,
  });

  const createFeatureMutation = useMutation({
    mutationFn: createFeature,
    onSuccess: (response) => {
      setCreateFeatureFor(null);
      if (response.project_id && response.feature_id) {
        setView({ kind: "feature", projectId: response.project_id, featureId: response.feature_id });
      }
    },
    onError: reportError,
  });

  const startFeatureMutation = useMutation({
    mutationFn: ({ target, approved }: { target: FeatureTarget; approved: boolean }) =>
      startFeature(target, approved),
    onSuccess: () => setPendingStart(null),
    onError: (err, variables) => {
      const error = asGuiError(err);
      if (error.kind === "needs_approval" && !variables.approved) {
        setPendingStart({ target: variables.target, message: error.message });
      } else {
        setPendingStart(null);
        reportError(error);
      }
    },
  });

  const stopFeatureMutation = useMutation({
    mutationFn: stopFeature,
    onError: reportError,
  });

  const lifecycle = (projectId: string, feature: Feature) => {
    const target = { project_id: projectId, feature_id: feature.id };
    return {
      starting: startFeatureMutation.isPending
        && startFeatureMutation.variables?.target.feature_id === feature.id,
      stopping: stopFeatureMutation.isPending
        && stopFeatureMutation.variables?.feature_id === feature.id,
      onStart: () => startFeatureMutation.mutate({ target, approved: false }),
      onStop: () => stopFeatureMutation.mutate(target),
    };
  };

  const activePlan = plan.data?.active ?? null;
  const createFeatureProject = projects.find((project) => project.id === createFeatureFor);

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">A</span>
          <span className="brand-name">Agent Mainframe</span>
        </div>

        <nav className="nav" aria-label="Workspace">
          <button
            className={view?.kind === "todos" ? "nav-item nav-item-active" : "nav-item"}
            onClick={() => setView({ kind: "todos" })}
          >
            <Icon name="inbox" />
            <span className="nav-label">Global TODOs</span>
          </button>

          <div className="nav-section">
            <span>Projects</span>
            <button
              className="btn btn-icon btn-ghost btn-sm"
              aria-label="New project"
              title="New project"
              onClick={() => setShowCreateProject(true)}
            >
              <Icon name="plus" />
            </button>
          </div>

          {workspace.isLoading && <p className="nav-note">Loading…</p>}
          {workspace.error && (
            <p role="alert" className="nav-note error-text">
              Failed to reach the backend: {asGuiError(workspace.error).message}
            </p>
          )}
          {workspace.data && projects.length === 0 && (
            <p className="nav-note">No projects yet.</p>
          )}

          {projects.map((project) => {
            const isCollapsed = collapsed[project.id] ?? false;
            const projectActive = view?.kind === "project" && view.projectId === project.id;
            return (
              <div key={project.id} className="nav-group">
                <div className={projectActive ? "nav-item nav-item-active" : "nav-item"}>
                  <button
                    className="nav-chevron"
                    aria-label={isCollapsed ? `Expand ${project.name}` : `Collapse ${project.name}`}
                    onClick={() => setCollapsed((current) => ({ ...current, [project.id]: !isCollapsed }))}
                  >
                    <Icon name={isCollapsed ? "chevronRight" : "chevronDown"} size={14} />
                  </button>
                  <button
                    className="nav-link"
                    onClick={() => setView({ kind: "project", projectId: project.id })}
                  >
                    <span className="nav-label">{project.name}</span>
                    <span className="nav-count">{project.features.length}</span>
                  </button>
                </div>
                {!isCollapsed && byStatus(project.features).map((feature) => {
                  const active = view?.kind === "feature" && view.featureId === feature.id;
                  return (
                    <button
                      key={feature.id}
                      className={active ? "nav-item nav-sub nav-item-active" : "nav-item nav-sub"}
                      onClick={() => setView({ kind: "feature", projectId: project.id, featureId: feature.id })}
                      title={`${feature.name} — ${feature.status}`}
                    >
                      <StatusDot status={feature.status} />
                      <span className="nav-label">{feature.name}</span>
                    </button>
                  );
                })}
              </div>
            );
          })}
        </nav>

        {activePlan && planMinimized && (
          <button className="plan-pill" onClick={() => setPlanMinimized(false)}>
            {LOADING_PHASES.has(activePlan.phase) ? <Spinner /> : <Icon name="sparkles" />}
            <span>
              <strong>{activePlan.kind === "quick" ? "Quick Plan" : "Plan interview"}</strong>
              <span className="muted">{activePlan.feature_name}</span>
            </span>
            <span className="plan-pill-open">Open</span>
          </button>
        )}
      </aside>

      <main className="main">
        {view?.kind === "todos" && (
          <div className="page">
            <PageHeader
              icon="inbox"
              title="Global TODOs"
              subtitle="Work that isn't tied to one project. Start it in any feature."
            />
            <div className="page-body page-narrow">
              <TodoPanel
                scope={{ kind: "global" }}
                title="TODOs"
                destinations={todoDestinations}
                launchTargets={todoAgentTargets}
                onLaunch={(todoId, target) => launchTodoAgent(todoId, target, false)}
                onPlan={beginTodoPlan}
                {...newFeatureHandlers(gitProjectIds)}
                onError={reportError}
              />
            </div>
          </div>
        )}

        {view?.kind === "project" && selectedProject && (
          <ProjectView
            project={selectedProject}
            harnessName={harnessName}
            modeName={modeName}
            lifecycle={(feature) => lifecycle(selectedProject.id, feature)}
            onOpenFeature={(feature) =>
              setView({ kind: "feature", projectId: selectedProject.id, featureId: feature.id })}
            onNewFeature={() => setCreateFeatureFor(selectedProject.id)}
            todoPanel={
              <TodoPanel
                scope={{ kind: "project", project_id: selectedProject.id }}
                title="Project TODOs"
                destinations={todoDestinations}
                launchTargets={todoAgentTargets.filter((candidate) => candidate.target.project_id === selectedProject.id)}
                onLaunch={(todoId, target) => launchTodoAgent(todoId, target, false)}
                onPlan={beginTodoPlan}
                {...newFeatureHandlers(selectedProject.is_git ? [selectedProject.id] : [])}
                onError={reportError}
              />
            }
          />
        )}

        {view?.kind === "feature" && selectedProject && selectedFeature && (
          <FeatureView
            project={selectedProject}
            feature={selectedFeature}
            harnessName={harnessName}
            modeName={modeName}
            tab={tabByFeature[selectedFeature.id]}
            onTab={(tab) => setTabByFeature((current) => ({ ...current, [selectedFeature.id]: tab }))}
            onBack={() => setView({ kind: "project", projectId: selectedProject.id })}
            onPlan={(quick) => void beginPlan(
              { project_id: selectedProject.id, feature_id: selectedFeature.id },
              quick,
            )}
            {...lifecycle(selectedProject.id, selectedFeature)}
            draft={draft}
            onDraftChange={(text) => setDraft((current) => current && { ...current, text })}
            onDiscardDraft={() => setDraft(null)}
            sendingPrompt={sendingPrompt}
            onSendDraft={(target, text) => {
              setSendingPrompt(true);
              void (async () => {
                try {
                  await terminalSubmitPrompt(target, text);
                  setDraft(null);
                } catch (err) {
                  reportError(err);
                } finally {
                  setSendingPrompt(false);
                }
              })();
            }}
            todoPanel={
              <TodoPanel
                scope={{ kind: "worktree", project_id: selectedProject.id, feature_id: selectedFeature.id }}
                hostFeatureId={selectedFeature.id}
                title="Worktree TODOs"
                description="TODOs for this checkout."
                destinations={todoDestinations}
                launchTargets={[{
                  label: selectedFeature.name,
                  target: { project_id: selectedProject.id, feature_id: selectedFeature.id },
                }]}
                onLaunch={(todoId, target) => launchTodoAgent(todoId, target, false)}
                onPlan={beginTodoPlan}
                {...newFeatureHandlers(selectedProject.is_git ? [selectedProject.id] : [])}
                onError={reportError}
              />
            }
          />
        )}

        {workspace.data && projects.length === 0 && !view && (
          <EmptyState
            icon="folder"
            title="Welcome to AMF"
            action={
              <button className="btn btn-primary" onClick={() => setShowCreateProject(true)}>
                <Icon name="plus" /> Add a project
              </button>
            }
          >
            Add a repository to start running agents in features and worktrees.
          </EmptyState>
        )}
      </main>

      {showCreateProject && (
        <CreateProjectForm
          agents={harnesses.data ?? []}
          pending={createProjectMutation.isPending}
          onCancel={() => setShowCreateProject(false)}
          onSubmit={(request) => createProjectMutation.mutate(request)}
        />
      )}

      {createFeatureProject && (
        <CreateFeatureForm
          projectName={createFeatureProject.name}
          agents={harnesses.data ?? []}
          modes={modes.data ?? []}
          isGit={createFeatureProject.is_git}
          defaultUseWorktree={createFeatureProject.is_git && createFeatureProject.features.length > 0}
          pending={createFeatureMutation.isPending || planBusy}
          onCancel={() => setCreateFeatureFor(null)}
          onSubmit={(request, planKind) => {
            const fullRequest = { ...request, project_name: createFeatureProject.name };
            if (planKind === "none") {
              createFeatureMutation.mutate(fullRequest);
            } else {
              void runPlanBegin(() => planBeginCreation(fullRequest, planKind === "quick"))
                .then((started) => { if (started) setCreateFeatureFor(null); });
            }
          }}
        />
      )}

      {pendingTodoNew && (
        <TodoNewFeatureForm
          kind={pendingTodoNew.kind}
          todoTitle={pendingTodoNew.title}
          projects={projects.filter((project) => pendingTodoNew.projectIds.includes(project.id))}
          agents={harnesses.data ?? []}
          modes={modes.data ?? []}
          pending={planBusy || todoNewBusy || pendingTodoNewApproval !== null}
          onCancel={() => setPendingTodoNew(null)}
          onSubmit={(request) => {
            if (pendingTodoNew.kind === "plan") {
              void runPlanBegin(() => planBeginTodoNew(pendingTodoNew.todoId, request))
                .then((started) => { if (started) setPendingTodoNew(null); });
            } else {
              void launchTodoNewFeature(pendingTodoNew.todoId, request, false);
            }
          }}
        />
      )}

      {activePlan && (
        <div hidden={planMinimized}>
          <Modal
            label="Plan interview"
            title={activePlan.kind === "quick" ? "Quick Plan" : "Plan interview"}
            subtitle={activePlan.feature_name}
            size="lg"
            onClose={() => setPlanMinimized(true)}
            dismissable={false}
            headerActions={
              <button
                className="btn btn-sm btn-ghost"
                onClick={() => setPlanMinimized(true)}
                title="Keep the interview running and hide this window"
              >
                <Icon name="minimize" /> Minimize
              </button>
            }
          >
            <PlanPanel
              view={activePlan}
              precall={plan.data?.precall ?? null}
              busy={planBusy}
              onAct={actOnPlan}
            />
          </Modal>
        </div>
      )}

      {pendingTodoNewApproval && (
        <ApprovalDialog
          label="Approve new TODO feature"
          title="Start another agent?"
          message={pendingTodoNewApproval.message}
          confirmLabel="Start anyway"
          busy={todoNewBusy}
          onConfirm={() => void launchTodoNewFeature(pendingTodoNewApproval.todoId, pendingTodoNewApproval.request, true)}
          onCancel={() => setPendingTodoNewApproval(null)}
        />
      )}

      {pendingPlanApproval && (
        <ApprovalDialog
          label="Approve planned agent"
          title="Start the planned agent?"
          message={pendingPlanApproval}
          confirmLabel="Accept plan and start agent"
          cancelLabel="Keep reviewing"
          busy={planBusy}
          onConfirm={() => void actOnPlan("accept_approved")}
          onCancel={() => setPendingPlanApproval(null)}
        />
      )}

      {pendingStart && (
        <ApprovalDialog
          label="Approve agent start"
          title="Start another agent?"
          message={pendingStart.message}
          confirmLabel="Start anyway"
          busy={startFeatureMutation.isPending}
          onConfirm={() => startFeatureMutation.mutate({ target: pendingStart.target, approved: true })}
          onCancel={() => setPendingStart(null)}
        />
      )}

      {pendingTodoLaunch && (
        <ApprovalDialog
          label="Approve TODO agent start"
          title="Start another agent?"
          message={pendingTodoLaunch.message}
          confirmLabel="Start anyway"
          onConfirm={() => void launchTodoAgent(pendingTodoLaunch.todoId, pendingTodoLaunch.target, true)}
          onCancel={() => setPendingTodoLaunch(null)}
        />
      )}

      <Toasts toasts={toasts} onDismiss={dismissToast} />
    </div>
  );
}

function PageHeader({
  icon,
  title,
  subtitle,
  badge,
  actions,
  crumb,
}: {
  icon?: IconName;
  title: string;
  subtitle?: ReactNode;
  badge?: ReactNode;
  actions?: ReactNode;
  crumb?: ReactNode;
}) {
  return (
    <header className="page-header">
      <div className="page-title">
        {crumb && <div className="crumb">{crumb}</div>}
        <div className="page-title-row">
          {icon && <span className="page-icon"><Icon name={icon} size={18} /></span>}
          <h1>{title}</h1>
          {badge}
        </div>
        {subtitle && <div className="page-subtitle">{subtitle}</div>}
      </div>
      {actions && <div className="row">{actions}</div>}
    </header>
  );
}

interface Lifecycle {
  starting: boolean;
  stopping: boolean;
  onStart: () => void;
  onStop: () => void;
}

function StartStopButton({ status, starting, stopping, onStart, onStop, size }: Lifecycle & {
  status: Feature["status"];
  size?: "sm";
}) {
  // In lists every row has one of these, so they stay quiet; the feature
  // header's single Start is the primary action on its page.
  const sm = size === "sm" ? " btn-sm" : "";
  return status === "stopped" ? (
    <button className={`btn ${size === "sm" ? "btn-secondary" : "btn-primary"}${sm}`} onClick={onStart} disabled={starting}>
      {starting ? <Spinner /> : <Icon name="play" size={12} />}
      {starting ? "Starting…" : "Start"}
    </button>
  ) : (
    <button className={`btn ${size === "sm" ? "btn-ghost" : "btn-secondary"}${sm}`} onClick={onStop} disabled={stopping}>
      {stopping ? <Spinner /> : <Icon name="stop" size={12} />}
      {stopping ? "Stopping…" : "Stop"}
    </button>
  );
}

function ProjectView({
  project,
  harnessName,
  modeName,
  lifecycle,
  onOpenFeature,
  onNewFeature,
  todoPanel,
}: {
  project: Project;
  harnessName: (slug: AgentSlug) => string;
  modeName: (slug: ModeSlug) => string;
  lifecycle: (feature: Feature) => Lifecycle;
  onOpenFeature: (feature: Feature) => void;
  onNewFeature: () => void;
  todoPanel: ReactNode;
}) {
  const running = project.features.filter((feature) => feature.status !== "stopped").length;
  return (
    <div className="page">
      <PageHeader
        icon="folder"
        title={project.name}
        subtitle={
          <>
            <span className="mono">{project.repo}</span>
            {project.is_git && <span className="tag">git</span>}
          </>
        }
        actions={
          <button className="btn btn-primary" onClick={onNewFeature}>
            <Icon name="plus" /> New feature
          </button>
        }
      />
      <div className="page-body project-grid">
        <section className="card" aria-label="Features">
          <header className="panel-header">
            <div>
              <h3>Features</h3>
            </div>
            {project.features.length > 0 && (
              <span className="count-pill">{running} of {project.features.length} running</span>
            )}
          </header>
          {project.features.length === 0 ? (
            <EmptyState
              icon="branch"
              title="No features yet"
              action={
                <button className="btn btn-secondary" onClick={onNewFeature}>
                  <Icon name="plus" /> New feature
                </button>
              }
            >
              A feature is a branch with its own agent sessions.
            </EmptyState>
          ) : (
            <ul className="feature-list">
              {byStatus(project.features).map((feature) => (
                <li key={feature.id} className="feature-row">
                  <button className="feature-open" onClick={() => onOpenFeature(feature)}>
                    <StatusDot status={feature.status} />
                    <span className="feature-text">
                      <span className="feature-name">{feature.name}</span>
                      <span className="feature-meta">
                        <span className="mono">{feature.branch}</span>
                        <span className="dot-sep" />
                        {harnessName(feature.agent)}
                        <span className="dot-sep" />
                        {modeName(feature.mode)}
                        {feature.sessions.length > 0 && (
                          <>
                            <span className="dot-sep" />
                            {feature.sessions.length} session{feature.sessions.length === 1 ? "" : "s"}
                          </>
                        )}
                      </span>
                    </span>
                  </button>
                  <StartStopButton status={feature.status} size="sm" {...lifecycle(feature)} />
                </li>
              ))}
            </ul>
          )}
        </section>
        <div className="card">{todoPanel}</div>
      </div>
    </div>
  );
}

function FeatureView({
  project,
  feature,
  harnessName,
  modeName,
  tab,
  onTab,
  onBack,
  onPlan,
  starting,
  stopping,
  onStart,
  onStop,
  draft,
  onDraftChange,
  onDiscardDraft,
  sendingPrompt,
  onSendDraft,
  todoPanel,
}: Lifecycle & {
  project: Project;
  feature: Feature;
  harnessName: (slug: AgentSlug) => string;
  modeName: (slug: ModeSlug) => string;
  tab: string | undefined;
  onTab: (tab: string) => void;
  onBack: () => void;
  onPlan: (quick: boolean) => void;
  draft: Draft | null;
  onDraftChange: (text: string) => void;
  onDiscardDraft: () => void;
  sendingPrompt: boolean;
  onSendDraft: (target: SessionTarget, text: string) => void;
  todoPanel: ReactNode;
}) {
  const sessions = feature.sessions.filter((session) => session.kind !== "todos");
  const activeTab = tab ?? sessions[0]?.id ?? TODOS_TAB;
  // A session just created by a launch may not be in the snapshot yet; keep
  // its tab visible so the handoff lands somewhere.
  const pendingSession = activeTab !== TODOS_TAB && !sessions.some((session) => session.id === activeTab);
  const isStopped = feature.status === "stopped";
  const target: SessionTarget | null = activeTab === TODOS_TAB ? null : {
    project_id: project.id,
    feature_id: feature.id,
    session_id: activeTab,
  };
  const activeDraft = target && draft?.key === sessionKey(target) ? draft : null;

  return (
    <div className="page page-fill">
      <PageHeader
        crumb={<button className="link-button" onClick={onBack}>{project.name}</button>}
        title={feature.name}
        badge={<StatusBadge status={feature.status} />}
        subtitle={
          <>
            <Icon name="branch" size={13} />
            <span className="mono">{feature.branch}</span>
            <span className="dot-sep" />
            {harnessName(feature.agent)}
            <span className="dot-sep" />
            {modeName(feature.mode)}
            {feature.is_worktree && (
              <>
                <span className="dot-sep" />
                <span className="mono truncate" title={feature.workdir}>{feature.workdir}</span>
              </>
            )}
          </>
        }
        actions={
          <>
            <Menu
              label="Plan"
              icon="sparkles"
              text="Plan"
              className="btn btn-secondary"
              items={[
                { label: "Plan interview", icon: "sparkles", hint: "Full", onSelect: () => onPlan(false) },
                { label: "Quick Plan", icon: "zap", hint: "Fast", onSelect: () => onPlan(true) },
              ]}
            />
            <StartStopButton
              status={feature.status}
              starting={starting}
              stopping={stopping}
              onStart={onStart}
              onStop={onStop}
            />
          </>
        }
      />

      <div className="tabs" role="tablist" aria-label="Feature sessions">
        {sessions.map((session) => (
          <button
            key={session.id}
            role="tab"
            aria-selected={activeTab === session.id}
            className={activeTab === session.id ? "tab tab-active" : "tab"}
            onClick={() => onTab(session.id)}
            title={session.kind}
          >
            <Icon name="terminal" size={14} />
            {session.label}
          </button>
        ))}
        {pendingSession && (
          <button role="tab" aria-selected className="tab tab-active">
            <Icon name="terminal" size={14} /> Agent
          </button>
        )}
        <span className="tabs-spacer" />
        <button
          role="tab"
          aria-selected={activeTab === TODOS_TAB}
          className={activeTab === TODOS_TAB ? "tab tab-active" : "tab"}
          onClick={() => onTab(TODOS_TAB)}
        >
          <Icon name="list" size={14} /> TODOs
        </button>
      </div>

      <div className="tab-body">
        {activeTab === TODOS_TAB && (
          <div className="page-narrow scroll">{todoPanel}</div>
        )}
        {target && isStopped && (
          <EmptyState
            icon="terminal"
            title="This feature is stopped"
            action={
              <button className="btn btn-primary" onClick={onStart} disabled={starting}>
                {starting ? <Spinner /> : <Icon name="play" size={12} />}
                {starting ? "Starting…" : "Start feature"}
              </button>
            }
          >
            Start it to attach to its agent sessions.
          </EmptyState>
        )}
        {target && !isStopped && (
          <div className="session">
            <TerminalPane key={sessionKey(target)} target={target} />
            {activeDraft && (
              <div className="composer">
                <div className="composer-head">
                  <Icon name="sparkles" />
                  <strong>Draft prompt</strong>
                  <span className="muted small">Review and edit before sending.</span>
                  <span className="tabs-spacer" />
                  <button className="btn btn-sm btn-ghost" onClick={onDiscardDraft}>Discard</button>
                </div>
                <textarea
                  aria-label="Draft prompt"
                  value={activeDraft.text}
                  onChange={(event) => onDraftChange(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && activeDraft.text.trim()) {
                      event.preventDefault();
                      onSendDraft(target, activeDraft.text);
                    }
                  }}
                  rows={5}
                />
                <div className="composer-foot">
                  <span className="muted small"><kbd>Ctrl</kbd> + <kbd>Enter</kbd> to send</span>
                  <button
                    className="btn btn-primary"
                    disabled={sendingPrompt || !activeDraft.text.trim()}
                    onClick={() => onSendDraft(target, activeDraft.text)}
                  >
                    {sendingPrompt ? <Spinner /> : <Icon name="send" />}
                    {sendingPrompt ? "Sending…" : "Send prompt"}
                  </button>
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function CreateProjectForm({
  agents,
  pending,
  onCancel,
  onSubmit,
}: {
  agents: HarnessInfo[];
  pending: boolean;
  onCancel: () => void;
  onSubmit: (request: {
    path: string;
    project_name: string;
    preferred_agent: AgentSlug | null;
    dry_run: boolean;
  }) => void;
}) {
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [nameEdited, setNameEdited] = useState(false);
  const [agent, setAgent] = useState<AgentSlug | "">("");

  return (
    <Modal
      label="New project"
      title="New project"
      subtitle="Register a repository with AMF."
      onClose={onCancel}
      onSubmit={() => onSubmit({
        path: path.trim(),
        project_name: name.trim(),
        preferred_agent: agent || null,
        dry_run: false,
      })}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onCancel}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={pending}>
            {pending && <Spinner />}
            {pending ? "Creating…" : "Create project"}
          </button>
        </>
      }
    >
      <div className="form-stack">
        <Field label="Repository path">
          <input
            className="mono"
            value={path}
            onChange={(event) => {
              setPath(event.target.value);
              if (!nameEdited) {
                setName(event.target.value.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "");
              }
            }}
            placeholder="/home/you/code/my-repo"
            required
            autoFocus
          />
        </Field>
        <Field label="Project name">
          <input
            value={name}
            onChange={(event) => {
              setName(event.target.value);
              setNameEdited(true);
            }}
            required
          />
        </Field>
        <Field label="Preferred harness">
          <select value={agent} onChange={(event) => setAgent(event.target.value as AgentSlug)}>
            <option value="">Default</option>
            {agents.map((a) => (
              <option key={a.slug} value={a.slug}>{a.display_name}</option>
            ))}
          </select>
        </Field>
      </div>
    </Modal>
  );
}

type PlanKind = "none" | "quick" | "full";

function CreateFeatureForm({
  projectName,
  agents,
  modes,
  isGit,
  defaultUseWorktree,
  pending,
  onCancel,
  onSubmit,
}: {
  projectName: string;
  agents: HarnessInfo[];
  modes: ModeInfo[];
  isGit: boolean;
  defaultUseWorktree: boolean;
  pending: boolean;
  onCancel: () => void;
  onSubmit: (request: Omit<CreateFeatureRequest, "project_name"> & { hook_choice: null }, planKind: PlanKind) => void;
}) {
  const [branch, setBranch] = useState("");
  const [agent, setAgent] = useState<AgentSlug>(agents[0]?.slug ?? "claude");
  const [mode, setMode] = useState<ModeSlug>(modes[0]?.slug ?? "vibe");
  const [useWorktree, setUseWorktree] = useState(defaultUseWorktree);
  const [planKind, setPlanKind] = useState<PlanKind>("none");
  const modeInfo = modes.find((candidate) => candidate.slug === mode);

  return (
    <Modal
      label="New feature"
      title="New feature"
      subtitle={`in ${projectName}`}
      onClose={onCancel}
      onSubmit={() => onSubmit({
        branch: branch.trim(),
        agent,
        mode,
        review: false,
        plan_mode: planKind !== "none",
        create_terminal: false,
        use_worktree: useWorktree,
        enable_chrome: false,
        hook_choice: null,
        dry_run: false,
      }, planKind)}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onCancel}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={pending || !branch.trim()}>
            {pending && <Spinner />}
            {pending ? "Creating…" : planKind === "none" ? "Create feature" : "Create and plan"}
          </button>
        </>
      }
    >
      <div className="form-stack">
        <Field label="Branch / feature name">
          <input
            className="mono"
            value={branch}
            onChange={(event) => setBranch(event.target.value)}
            placeholder="fix-login-redirect"
            required
            autoFocus
          />
        </Field>
        <div className="form-row">
          <Field label="Harness">
            <select value={agent} onChange={(event) => setAgent(event.target.value as AgentSlug)}>
              {agents.map((a) => (
                <option key={a.slug} value={a.slug}>{a.display_name}</option>
              ))}
            </select>
          </Field>
          <Field label="Mode">
            <select value={mode} onChange={(event) => setMode(event.target.value as ModeSlug)}>
              {modes.map((m) => (
                <option key={m.slug} value={m.slug} title={m.description}>{m.display_name}</option>
              ))}
            </select>
          </Field>
        </div>
        {modeInfo?.description && <p className="field-hint mode-hint">{modeInfo.description}</p>}
        <Field
          label="Planning"
          hint={{
            none: "Start the agent right away.",
            quick: "A short triage: a question or two, then a plan only if the task needs one.",
            full: "A guided interview that produces a reviewed plan before the agent starts.",
          }[planKind]}
        >
          <Segmented
            label="Planning"
            value={planKind}
            onChange={setPlanKind}
            options={[
              { value: "none", label: "Start directly" },
              { value: "quick", label: "Quick Plan" },
              { value: "full", label: "Full Plan" },
            ]}
          />
        </Field>
        {isGit && (
          <Switch
            checked={useWorktree}
            onChange={setUseWorktree}
            label="Use a git worktree"
            hint="Gives the feature its own checkout so agents don't collide."
          />
        )}
      </div>
    </Modal>
  );
}

export function TodoNewFeatureForm({
  kind,
  todoTitle,
  projects,
  agents,
  modes,
  pending,
  onCancel,
  onSubmit,
}: {
  kind: "plan" | "launch";
  todoTitle: string;
  projects: Project[];
  agents: { slug: AgentSlug; display_name: string }[];
  modes: { slug: ModeSlug; display_name: string }[];
  pending: boolean;
  onCancel: () => void;
  onSubmit: (request: CreateFeatureRequest) => void;
}) {
  const suggested = todoTitle.toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40) || "todo-work";
  const [projectId, setProjectId] = useState(projects[0]?.id ?? "");
  const [branch, setBranch] = useState(suggested);
  const [agent, setAgent] = useState<AgentSlug>(agents[0]?.slug ?? "claude");
  const [mode, setMode] = useState<ModeSlug>(modes[0]?.slug ?? "vibe");
  const project = projects.find((candidate) => candidate.id === projectId) ?? projects[0];
  const selectedAgent = agents.some((candidate) => candidate.slug === agent)
    ? agent : (agents[0]?.slug ?? agent);
  const selectedMode = modes.some((candidate) => candidate.slug === mode)
    ? mode : (modes[0]?.slug ?? mode);

  return (
    <Modal
      label={kind === "plan" ? "Plan TODO in a new feature" : "Start TODO in a new feature"}
      title={kind === "plan" ? "Plan in a new feature" : "Start in a new feature"}
      subtitle={todoTitle}
      onClose={onCancel}
      onSubmit={() => {
        if (!project || !branch.trim()) return;
        onSubmit({
          project_name: project.name,
          branch: branch.trim(),
          agent: selectedAgent,
          mode: selectedMode,
          review: false,
          plan_mode: kind === "plan",
          create_terminal: false,
          use_worktree: true,
          enable_chrome: false,
          hook_choice: null,
          dry_run: false,
        });
      }}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onCancel}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={pending || !project || !branch.trim()}>
            {pending && <Spinner />}
            {kind === "plan" ? "Start plan" : "Create and start"}
          </button>
        </>
      }
    >
      <p className="muted small modal-lead">
        {kind === "plan"
          ? "The worktree and agent launch wait until you accept the plan."
          : "Creates a worktree and starts its agent. The TODO prompt opens as an editable draft."}
      </p>
      <div className="form-stack">
        {projects.length > 1 && (
          <Field label="Project">
            <select value={project?.id ?? ""} onChange={(event) => setProjectId(event.target.value)}>
              {projects.map((candidate) => (
                <option key={candidate.id} value={candidate.id}>{candidate.name}</option>
              ))}
            </select>
          </Field>
        )}
        <Field label="Branch">
          <input className="mono" value={branch} onChange={(event) => setBranch(event.target.value)} required autoFocus />
        </Field>
        <div className="form-row">
          <Field label="Harness">
            <select value={selectedAgent} onChange={(event) => setAgent(event.target.value as AgentSlug)}>
              {agents.map((candidate) => (
                <option key={candidate.slug} value={candidate.slug}>{candidate.display_name}</option>
              ))}
            </select>
          </Field>
          <Field label="Mode">
            <select value={selectedMode} onChange={(event) => setMode(event.target.value as ModeSlug)}>
              {modes.map((candidate) => (
                <option key={candidate.slug} value={candidate.slug}>{candidate.display_name}</option>
              ))}
            </select>
          </Field>
        </div>
      </div>
    </Modal>
  );
}
