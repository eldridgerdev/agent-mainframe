import { ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  LibraryScope,
  ReviewAction,
  ReviewView,
  reviewBegin,
  reviewAct,
  reviewSnapshot,
  reviewTakeCompletion,
  LearningView,
  LearningAction,
  learningBegin,
  learningSnapshot,
  learningAct,
  learningLaunchAgent,
  AgentSlug,
  NewSessionKind,
  NewSessionOption,
  CollapseTarget,
  CreateFeatureRequest,
  Feature,
  FeatureSession,
  FeatureTarget,
  GuiError,
  HarnessInfo,
  ModeInfo,
  ModeSlug,
  PlanAction,
  PlanInput,
  PlanStatus,
  Project,
  SavedAgentSession,
  SessionTarget,
  SessionRecoveryChoice,
  SessionRecoveryOption,
  TodoDeleteChoice,
  TodoHostChoice,
  TodoHostPrompt,
  WorkspaceSnapshot,
  asGuiError,
  addSession,
  createFeature,
  createProject,
  deleteFeature,
  getSnapshot,
  newSessionOptions,
  planAct,
  planBegin,
  planBeginCreation,
  planBeginTodoHost,
  planBeginTodoNew,
  planSnapshot,
  recoverSession,
  savedAgentSessions,
  sessionRecoveryOption,
  setCollapsed,
  startFeature,
  removeSession,
  startSession,
  stopFeature,
  stopSession,
  supportedHarnesses,
  supportedModes,
  terminalSubmitPrompt,
  todoLaunchAgent,
  todoLaunchNewFeature,
} from "./api";
import TerminalPane from "./TerminalPane";
import PromptComposer from "./PromptComposer";
import PromptLibraryPanel from "./PromptLibraryPanel";
import PromptOverridesPanel from "./PromptOverridesPanel";
import { OverrideContext, promptOverridesPrecallTarget } from "./promptOverridesApi";
import DormancyPanel from "./DormancyPanel";
import DiffPanel from "./DiffPanel";
import SupervisedEditsPanel, { SupervisedEditsPanelHandle, usePendingEdits } from "./SupervisedEditsPanel";
import PrTriagePanel from "./PrTriagePanel";
import ScreenshotsPanel from "./ScreenshotsPanel";
import ReviewPanel from "./ReviewPanel";
import TodoPanel, { TodoAgentTarget, TodoDestination } from "./TodoPanel";
import LearningPanel from "./LearningPanel";
import PlanPanel from "./PlanPanel";
import RecoveryDialog from "./RecoveryDialog";
import NewSessionDialog from "./NewSessionDialog";
import DeleteFeatureDialog from "./DeleteFeatureDialog";
import SidebarTree, { byStatus, withCollapsed } from "./SidebarTree";
import WorktreeHookField, { useWorktreeHookChoice } from "./WorktreeHookField";
import {
  SessionStartStopButton,
  SessionStateDot,
  closingStopsFeature,
  sessionRunning,
  stoppedSessionCount,
} from "./SessionControls";
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
// A session a launch just created can take a snapshot or two to appear. Only
// that launch's session is shown ahead of the snapshot, and only for this
// long, so a session removed elsewhere (killed from the TUI, say) can't leave
// a phantom tab that keeps trying to attach.
const PENDING_SESSION_GRACE_MS = 10_000;

type View =
  | { kind: "todos" }
  | { kind: "project"; projectId: string }
  | { kind: "feature"; projectId: string; featureId: string };

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

const sessionKey = (target: { feature_id: string; session_id: string }) =>
  `${target.feature_id}:${target.session_id}`;

// Joins a handoff's seed onto a draft that already has unsent text.
const DRAFT_SEPARATOR = "\n\n";

/** Drops entries whose key is not in `live`, keeping the same object when nothing goes. */
function pruneKeys<T>(current: Record<string, T>, live: Set<string>): Record<string, T> {
  const stale = Object.keys(current).filter((key) => !live.has(key));
  if (stale.length === 0) return current;
  const next = { ...current };
  for (const key of stale) delete next[key];
  return next;
}

/// Workspace shell: sidebar navigation (global TODOs, projects, features),
/// a main view for the selection, and modals for creation, approvals and
/// the plan interview. Id resolution, idempotency, and terminal correctness
/// have their own Rust-side coverage; this file is the UI wiring.
export default function App() {
  const queryClient = useQueryClient();
  const [view, setView] = useState<View | null>(null);
  const [showCreateProject, setShowCreateProject] = useState(false);
  const [createFeatureFor, setCreateFeatureFor] = useState<string | null>(null);
  const [tabByFeature, setTabByFeature] = useState<Record<string, string>>({});
  const [pendingSessionByFeature, setPendingSessionByFeature] = useState<Record<string, string>>({});
  const [learning, setLearning] = useState<LearningView | null>(null);
  const [learningBusy, setLearningBusy] = useState(false);
  const learningActionPending = useRef(false);
  const [learningApproval, setLearningApproval] = useState<{ qaId: string; message: string } | null>(null);
  const [promptLibrary, setPromptLibrary] = useState<{ scope: LibraryScope; target: SessionTarget | null } | null>(null);
  const [promptOverrides, setPromptOverrides] = useState<{
    context: OverrideContext; promptId: string | null; harness: AgentSlug | null; fromPrecall: boolean;
    contextNote?: string | null;
  } | null>(null);
  const [showDormancy, setShowDormancy] = useState(false);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [sendingPrompts, setSendingPrompts] = useState<Record<string, boolean>>({});
  const promptSendsInFlight = useRef(new Set<string>());
  const [composerFocusKey, setComposerFocusKey] = useState<string | null>(null);
  const composerFocusHandled = useCallback(() => setComposerFocusKey(null), []);
  const [planMinimized, setPlanMinimized] = useState(false);
  const [pendingPlanApproval, setPendingPlanApproval] = useState<string | null>(null);
  const [recoveryDialog, setRecoveryDialog] = useState<{
    target: SessionTarget;
    option: SessionRecoveryOption;
    sessions: SavedAgentSession[] | null;
    loading: boolean;
    selectedId: string | null;
  } | null>(null);
  const [recoveryChecking, setRecoveryChecking] = useState(false);
  const [pendingRecoveryApproval, setPendingRecoveryApproval] = useState<{
    target: SessionTarget;
    choice: SessionRecoveryChoice;
    pickedId: string | null;
    message: string;
  } | null>(null);
  const [newSessionDialog, setNewSessionDialog] = useState<{
    target: FeatureTarget;
    preferredKind: NewSessionKind;
    options: NewSessionOption[];
  } | null>(null);
  const [newSessionLoading, setNewSessionLoading] = useState(false);
  const [pendingAddSessionApproval, setPendingAddSessionApproval] = useState<{
    target: FeatureTarget;
    kind: NewSessionKind;
    label: string | null;
    message: string;
  } | null>(null);
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
  // `isPending` reaches the dialog a render late, so a double-click (or
  // Enter then click) would otherwise send two launches and two sessions.
  const addSessionInFlight = useRef(false);
  const [closeSessionDialog, setCloseSessionDialog] = useState<{
    target: SessionTarget;
    label: string;
    last: boolean;
  } | null>(null);
  const removeSessionInFlight = useRef(false);
  const [pendingSessionStart, setPendingSessionStart] = useState<{
    target: SessionTarget;
    message: string;
  } | null>(null);
  const [diffTarget, setDiffTarget] = useState<FeatureTarget | null>(null);
  const [supervisedTarget, setSupervisedTarget] = useState<FeatureTarget | null>(null);
  const supervisedPanel = useRef<SupervisedEditsPanelHandle>(null);
  function openSupervisedEdits(target: FeatureTarget) {
    if (supervisedTarget?.project_id === target.project_id && supervisedTarget.feature_id === target.feature_id) return;
    const proceed = () => setSupervisedTarget(target);
    if (supervisedPanel.current) supervisedPanel.current.requestSwitch(proceed);
    else proceed();
  }
  const [screenshotTarget, setScreenshotTarget] = useState<{ target: FeatureTarget | null; sessionId: string | null } | null>(null);
  const [prTriageTarget, setPrTriageTarget] = useState<FeatureTarget | null>(null);
  const [review, setReview] = useState<ReviewView | null>(null);
  const [reviewBusy, setReviewBusy] = useState(false);
  const [reviewError, setReviewError] = useState<string | null>(null);
  const reviewPending = useRef(false);
  const [deleteFeatureDialog, setDeleteFeatureDialog] = useState<{
    target: FeatureTarget;
    projectName: string;
    featureName: string;
    isWorktree: boolean;
    unfinished: number | null;
    todoHost?: TodoHostPrompt;
  } | null>(null);
  const deleteFeatureInFlight = useRef(false);
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

  useEffect(() => {
    if (!workspace.data) return;
    const exists = (target: FeatureTarget) => projects.some((project) =>
      project.id === target.project_id && project.features.some((feature) => feature.id === target.feature_id));
    if (screenshotTarget?.target && !exists(screenshotTarget.target)) setScreenshotTarget(null);
    if (prTriageTarget && !exists(prTriageTarget)) setPrTriageTarget(null);
  }, [workspace.data, projects, screenshotTarget, prTriageTarget]);

  // A launched session that has reached the snapshot no longer needs the
  // grace period: from here on it is shown only while the snapshot has it.
  useEffect(() => {
    setPendingSessionByFeature((current) => {
      const arrived = Object.entries(current).filter(([featureId, sessionId]) =>
        projects.some((project) => project.features.some((feature) =>
          feature.id === featureId && feature.sessions.some((session) => session.id === sessionId))));
      if (arrived.length === 0) return current;
      const next = { ...current };
      for (const [featureId] of arrived) delete next[featureId];
      return next;
    });
  }, [projects]);

  // A closed session or deleted feature takes its unsent draft with it, so a
  // reused key can never resurrect stale text. A just-launched session the
  // snapshot hasn't caught up with yet keeps its handoff draft.
  const workspaceData = workspace.data;
  useEffect(() => {
    if (!workspaceData) return;
    const live = new Set(workspaceData.projects.flatMap((project) => project.features.flatMap(
      (feature) => feature.sessions.map((session) =>
        sessionKey({ feature_id: feature.id, session_id: session.id })))));
    for (const [featureId, sessionId] of Object.entries(pendingSessionByFeature)) {
      live.add(sessionKey({ feature_id: featureId, session_id: sessionId }));
    }
    setDrafts((current) => pruneKeys(current, live));
    setSendingPrompts((current) => pruneKeys(current, live));
    setComposerFocusKey((current) => current !== null && !live.has(current) ? null : current);
  }, [workspaceData, pendingSessionByFeature]);

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

  /** Collapse or expand a tree row in the store shared with the TUI, showing
   *  the change at once and reloading the store's truth if the write is
   *  refused. */
  const toggleCollapsed = useCallback(async (target: CollapseTarget, collapsed: boolean) => {
    // A poll already in flight read the store before this write and would
    // flip the row back when it lands.
    await queryClient.cancelQueries({ queryKey: SNAPSHOT_KEY });
    queryClient.setQueryData<WorkspaceSnapshot>(
      SNAPSHOT_KEY, (current) => current && withCollapsed(current, target, collapsed));
    try {
      queryClient.setQueryData(SNAPSHOT_KEY, await setCollapsed(target, collapsed));
    } catch (err) {
      // Not `!collapsed`: a second click may have landed since this one.
      void queryClient.invalidateQueries({ queryKey: SNAPSHOT_KEY });
      reportError(err);
    }
  }, [queryClient, reportError]);

  const pendingEdits = usePendingEdits(pushToast, openSupervisedEdits);

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
    setPendingSessionByFeature((current) => ({ ...current, [target.feature_id]: target.session_id }));
    window.setTimeout(() => setPendingSessionByFeature((current) => {
      if (current[target.feature_id] !== target.session_id) return current;
      const next = { ...current };
      delete next[target.feature_id];
      return next;
    }), PENDING_SESSION_GRACE_MS);
    if (draftPrompt !== undefined) {
      const key = sessionKey(target);
      setDrafts((current) => ({
        ...current,
        // A handoff to an existing session must preserve its unsent message.
        [key]: current[key] ? `${current[key]}${DRAFT_SEPARATOR}${draftPrompt}` : draftPrompt,
      }));
      setComposerFocusKey(key);
    }
  }

  function openPromptLibrary(target: SessionTarget | null = null) {
    const scope: LibraryScope = target
      ? { kind: "feature", project_id: target.project_id, feature_id: target.feature_id }
      : selectedFeature && selectedProject
        ? { kind: "feature", project_id: selectedProject.id, feature_id: selectedFeature.id }
        : selectedProject ? { kind: "project", project_id: selectedProject.id } : { kind: "global" };
    setPromptLibrary({ scope, target });
  }

  function openPromptOverrides() {
    const context: OverrideContext = selectedFeature && selectedProject
      ? { kind: "feature", project_id: selectedProject.id, feature_id: selectedFeature.id }
      : selectedProject ? { kind: "project", project_id: selectedProject.id } : { kind: "global" };
    setPromptOverrides({ context, promptId: null, harness: null, fromPrecall: false });
  }

  /** The pre-call notice's "Edit prompt": the pending call keeps waiting. */
  async function editPrecallPrompt() {
    try {
      const target = await promptOverridesPrecallTarget();
      setPromptOverrides({
        context: target.context, promptId: target.prompt_id, harness: target.harness, fromPrecall: true,
        contextNote: target.context_note,
      });
    } catch (err) {
      reportError(err);
    }
  }

  function insertLibraryPrompt(target: SessionTarget, text: string) {
    const project = queryClient.getQueryData<WorkspaceSnapshot>(SNAPSHOT_KEY)?.projects.find((project) => project.id === target.project_id);
    const feature = project?.features.find((feature) => feature.id === target.feature_id);
    if (!feature?.sessions.some((session) => session.id === target.session_id)) {
      throw { kind: "not_found", message: "That session was removed. Choose another agent session." };
    }
    openSession(target, text);
    setPromptLibrary(null);
  }

  function changeDraft(target: SessionTarget, text: string) {
    const key = sessionKey(target);
    if (promptSendsInFlight.current.has(key)) return;
    setDrafts((current) => ({ ...current, [key]: text }));
  }

  async function sendDraft(target: SessionTarget, text: string) {
    const key = sessionKey(target);
    if (!text.trim() || promptSendsInFlight.current.has(key)) return;
    // Lock synchronously: repeated keyboard/click events can arrive before
    // React renders the pending state.
    promptSendsInFlight.current.add(key);
    setSendingPrompts((current) => ({ ...current, [key]: true }));
    try {
      await terminalSubmitPrompt(target, text);
      // Completion belongs to the originating session, even after navigation.
      // Edits are locked while sending, so the draft can only have grown by a
      // handoff appending its seed: remove the delivered text and keep that.
      setDrafts((current) => {
        const draft = current[key];
        if (draft === undefined || !draft.startsWith(text)) return current;
        const rest = draft.slice(text.length);
        return {
          ...current,
          [key]: rest.startsWith(DRAFT_SEPARATOR) ? rest.slice(DRAFT_SEPARATOR.length) : rest,
        };
      });
    } catch (err) {
      reportError(err);
    } finally {
      promptSendsInFlight.current.delete(key);
      setSendingPrompts((current) => {
        if (!(key in current)) return current;
        const next = { ...current };
        delete next[key];
        return next;
      });
    }
  }

  useEffect(() => {
    if (!learning || learningBusy) return;
    let cancelled = false;
    const workflowId = learning.workflow_id;
    const timer = window.setInterval(() => {
      void learningSnapshot().then((next) => {
        if (cancelled) return;
        setLearning((current) => {
          if (current?.workflow_id !== workflowId) return current;
          if (!next) return null;
          return next.workflow_id === current.workflow_id && next.revision >= current.revision ? next : current;
        });
      }).catch(() => { /* Explicit actions report deleted/stale targets. */ });
    }, 1000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [learning?.workflow_id, learningBusy]);

  async function beginLearning(target: FeatureTarget) {
    if (learningActionPending.current) return;
    learningActionPending.current = true;
    setLearningBusy(true);
    try { setLearning(await learningBegin(target)); }
    catch (err) { reportError(err); }
    finally { learningActionPending.current = false; setLearningBusy(false); }
  }

  // Poll while AI work or a project check is in flight, and nothing
  // else changes the review without an explicit action.
  useEffect(() => {
    if (!review || reviewBusy || (!review.ai.running && review.check?.status !== "running")) return;
    let cancelled = false;
    let polling = false;
    const workflowId = review.workflow_id;
    const timer = window.setInterval(() => {
      if (polling || reviewPending.current) return;
      polling = true;
      void reviewSnapshot(workflowId).then(async (next) => {
        // Null means the backend already closed this review. Always collect
        // its one-shot result, even if an action began or this effect reset.
        if (next === null) { await finishReview(workflowId); return; }
        if (cancelled || reviewPending.current) return;
        setReview((current) => current?.workflow_id === workflowId && next.workflow_id === workflowId
          && next.revision > current.revision ? next : current);
      }).catch(() => { /* Explicit actions report conflicts. */ }).finally(() => { polling = false; });
    }, 1000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [review?.workflow_id, review?.ai.running, review?.check?.status, reviewBusy]);

  async function beginReview(target: FeatureTarget) {
    if (reviewPending.current) return;
    reviewPending.current = true;
    setReviewBusy(true);
    setReviewError(null);
    try { setReview(await reviewBegin(target)); }
    catch (err) { reportError(err); }
    finally { reviewPending.current = false; setReviewBusy(false); }
  }

  /** Re-read the open review after a parser install so its files gain colours. */
  async function refreshReviewSyntax() {
    const workflowId = review?.workflow_id;
    if (!workflowId || reviewPending.current) return;
    try {
      const next = await reviewSnapshot(workflowId);
      if (next) setReview((current) => current?.workflow_id === workflowId && next.revision >= current.revision ? next : current);
    } catch { /* The next action reports a closed or changed review. */ }
  }

  /** Close a completed review and apply its handoff exactly once. */
  async function finishReview(workflowId: string) {
    const done = await reviewTakeCompletion(workflowId);
    setReview((current) => current?.workflow_id === workflowId ? null : current);
    if (!done) return;
    pushToast({ tone: "info", title: "Final Review", message: done.message });
    // A submitted prompt shows its agent working; an unsent one joins the
    // session's composer draft for the reviewer to edit and send.
    if (done.handoff) openSession(done.handoff.target, done.handoff.draft_prompt ?? undefined);
    void queryClient.invalidateQueries({ queryKey: SNAPSHOT_KEY });
  }

  async function actReview(action: ReviewAction): Promise<boolean> {
    if (!review || reviewPending.current) return false;
    reviewPending.current = true;
    setReviewBusy(true);
    setReviewError(null);
    try {
      const next = await reviewAct(review, action);
      if (next === null && action.kind === "complete") {
        await finishReview(review.workflow_id);
        return true;
      }
      setReview(next);
      // A failed pause returns the retained review with its save error.
      return !(action.kind === "pause" && next !== null);
    } catch (err) { setReviewError(asGuiError(err).message); return false; }
    finally { reviewPending.current = false; setReviewBusy(false); }
  }

  async function actLearning(action: LearningAction): Promise<boolean> {
    if (!learning || learningActionPending.current) return false;
    learningActionPending.current = true;
    setLearningBusy(true);
    try {
      setLearning(await learningAct(learning, action));
      if (action.kind === "keep_todo") void queryClient.invalidateQueries({ queryKey: ["todos"] });
      return true;
    }
    catch (err) { reportError(err); return false; }
    finally { learningActionPending.current = false; setLearningBusy(false); }
  }

  async function launchLearning(qaId: string, approved = false) {
    if (!learning || learningActionPending.current) return;
    learningActionPending.current = true;
    setLearningBusy(true);
    try {
      const handoff = await learningLaunchAgent(learning, qaId, approved);
      setLearning(null); setLearningApproval(null);
      openSession(handoff.target, handoff.draft_prompt);
      if (handoff.notice) pushToast({ tone: "error", title: "Learning", message: handoff.notice });
      if (handoff.info) pushToast({ tone: "info", title: "Learning", message: handoff.info });
      // A stopped linked session goes through the tab's own start, which
      // offers to resume its saved conversation.
      if (handoff.start_required) void beginSessionStart(handoff.target);
      void queryClient.invalidateQueries({ queryKey: SNAPSHOT_KEY });
    } catch (err) {
      const error = asGuiError(err);
      if (error.kind === "needs_approval") setLearningApproval({ qaId, message: error.message });
      else { setLearningApproval(null); reportError(err); }
    } finally { learningActionPending.current = false; setLearningBusy(false); }
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
      const status = await start();
      updatePlan(status);
      if (status.hook_warning) {
        pushToast({ tone: "info", title: "Plan", message: status.hook_warning });
      }
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

  const recoverSessionMutation = useMutation({
    mutationFn: ({ target, choice, pickedId, approved }: {
      target: SessionTarget;
      choice: SessionRecoveryChoice;
      pickedId: string | null;
      approved: boolean;
    }) => recoverSession(target, choice, pickedId, approved),
    onSuccess: () => {
      setRecoveryDialog(null);
      setPendingRecoveryApproval(null);
    },
    onError: (err, variables) => {
      const error = asGuiError(err);
      if (error.kind === "needs_approval" && !variables.approved) {
        setRecoveryDialog(null);
        setPendingRecoveryApproval({
          target: variables.target,
          choice: variables.choice,
          pickedId: variables.pickedId,
          message: error.message,
        });
      } else {
        setPendingRecoveryApproval(null);
        reportError(error);
      }
    },
  });

  const addSessionMutation = useMutation({
    mutationFn: ({ target, kind, label, approved }: {
      target: FeatureTarget;
      kind: NewSessionKind;
      label: string | null;
      approved: boolean;
    }) => addSession(target, kind, label, approved),
    onSettled: () => {
      addSessionInFlight.current = false;
    },
    onSuccess: (response) => {
      setNewSessionDialog(null);
      setPendingAddSessionApproval(null);
      openSession(response.target);
    },
    onError: (err, variables) => {
      const error = asGuiError(err);
      if (error.kind === "needs_approval" && !variables.approved) {
        setNewSessionDialog(null);
        setPendingAddSessionApproval({
          target: variables.target,
          kind: variables.kind,
          label: variables.label,
          message: error.message,
        });
      } else {
        setPendingAddSessionApproval(null);
        reportError(error);
      }
    },
  });

  function requestAddSession(variables: {
    target: FeatureTarget;
    kind: NewSessionKind;
    label: string | null;
    approved: boolean;
  }) {
    if (addSessionInFlight.current) return;
    addSessionInFlight.current = true;
    addSessionMutation.mutate(variables);
  }

  async function openNewSession(project: Project, feature: Feature) {
    const target = { project_id: project.id, feature_id: feature.id };
    setNewSessionLoading(true);
    try {
      const options = await newSessionOptions(target);
      setNewSessionDialog({ target, preferredKind: feature.agent, options });
    } catch (err) {
      reportError(err);
    } finally {
      setNewSessionLoading(false);
    }
  }

  const stopSessionMutation = useMutation({
    mutationFn: stopSession,
    onSuccess: (response) =>
      pushToast({ tone: "info", title: "Session stopped", message: response.message }),
    onError: reportError,
  });

  const startSessionMutation = useMutation({
    mutationFn: ({ target, approved }: { target: SessionTarget; approved: boolean }) =>
      startSession(target, approved),
    onSuccess: () => setPendingSessionStart(null),
    onError: (err, variables) => {
      const error = asGuiError(err);
      if (error.kind === "needs_approval" && !variables.approved) {
        setPendingSessionStart({ target: variables.target, message: error.message });
      } else {
        setPendingSessionStart(null);
        reportError(error);
      }
    },
  });

  /** Start a stopped tab -- the TUI's `Enter` on a session row: offer to
   * resume its saved conversation when it has one. In a stopped feature the
   * backend starts the feature around that session. */
  async function beginSessionStart(target: SessionTarget) {
    setRecoveryChecking(true);
    try {
      const option = await sessionRecoveryOption(target);
      if (option) {
        setRecoveryDialog({ target, option, sessions: null, loading: false, selectedId: null });
      } else {
        startSessionMutation.mutate({ target, approved: false });
      }
    } catch (err) {
      reportError(err);
    } finally {
      setRecoveryChecking(false);
    }
  }

  const sessionLifecycle = (projectId: string, feature: Feature) => (session: FeatureSession) => {
    const target = { project_id: projectId, feature_id: feature.id, session_id: session.id };
    return {
      starting: recoveryChecking
        || (startSessionMutation.isPending
          && startSessionMutation.variables?.target.session_id === session.id)
        || (recoverSessionMutation.isPending
          && recoverSessionMutation.variables?.target.session_id === session.id),
      stopping: stopSessionMutation.isPending
        && stopSessionMutation.variables?.session_id === session.id,
      onStart: () => void beginSessionStart(target),
      onStop: () => stopSessionMutation.mutate(target),
    };
  };

  const removeSessionMutation = useMutation({
    mutationFn: removeSession,
    onSettled: () => {
      removeSessionInFlight.current = false;
    },
    onSuccess: (response) => {
      setCloseSessionDialog(null);
      pushToast({ tone: "info", title: "Session closed", message: response.message });
    },
    onError: (err) => {
      setCloseSessionDialog(null);
      reportError(err);
    },
  });

  function requestRemoveSession(target: SessionTarget) {
    if (removeSessionInFlight.current) return;
    removeSessionInFlight.current = true;
    removeSessionMutation.mutate(target);
  }

  const deleteFeatureMutation = useMutation({
    mutationFn: ({ target, todos, todoHost }: { target: FeatureTarget; todos: TodoDeleteChoice | null; todoHost: TodoHostChoice | null }) =>
      deleteFeature(target, todos, todoHost),
    onSettled: () => {
      deleteFeatureInFlight.current = false;
    },
    onSuccess: (response, { target }) => {
      if (response.status === "needs_todo_disposition") {
        setDeleteFeatureDialog((current) => current && { ...current, unfinished: response.unfinished });
        return;
      }
      if (response.status === "needs_todo_host") {
        setDeleteFeatureDialog((current) => current && { ...current, todoHost: response.prompt });
        return;
      }
      setDeleteFeatureDialog(null);
      setView((current) => current?.kind === "feature" && current.featureId === target.feature_id
        ? { kind: "project", projectId: target.project_id }
        : current);
      setTabByFeature((current) => {
        const next = { ...current };
        delete next[target.feature_id];
        return next;
      });
      pushToast({ tone: "info", title: "Feature deleted", message: response.message });
    },
    onError: (err) => {
      setDeleteFeatureDialog(null);
      reportError(err);
    },
  });

  function requestDeleteFeature(target: FeatureTarget, todos: TodoDeleteChoice | null, todoHost: TodoHostChoice | null) {
    if (deleteFeatureInFlight.current) return;
    deleteFeatureInFlight.current = true;
    deleteFeatureMutation.mutate({ target, todos, todoHost });
  }

  const stopFeatureMutation = useMutation({
    mutationFn: stopFeature,
    onError: reportError,
  });

  /** The feature's Start, like the TUI's `c`: no resume prompt. Resuming is
   * asked per session, from a stopped tab (the TUI's `Enter` on a session). */
  const beginFeatureStart = (projectId: string, feature: Feature) =>
    startFeatureMutation.mutate({ target: { project_id: projectId, feature_id: feature.id }, approved: false });

  const lifecycle = (projectId: string, feature: Feature) => {
    const target = { project_id: projectId, feature_id: feature.id };
    return {
      starting: recoveryChecking || (startFeatureMutation.isPending
        && startFeatureMutation.variables?.target.feature_id === feature.id),
      stopping: stopFeatureMutation.isPending
        && stopFeatureMutation.variables?.feature_id === feature.id,
      onStart: () => void beginFeatureStart(projectId, feature),
      onStop: () => stopFeatureMutation.mutate(target),
    };
  };

  const loadRecoverySessions = async () => {
    if (!recoveryDialog) return;
    const target = recoveryDialog.target;
    setRecoveryDialog((current) => current && { ...current, loading: true });
    try {
      const sessions = await savedAgentSessions(target);
      setRecoveryDialog((current) => current && sessionKey(current.target) === sessionKey(target)
        ? { ...current, sessions, loading: false, selectedId: sessions[0]?.id ?? null }
        : current);
    } catch (err) {
      setRecoveryDialog((current) => current && { ...current, loading: false });
      reportError(err);
    }
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

          <button className="nav-item" onClick={() => openPromptLibrary()}>
            <Icon name="file" /><span className="nav-label">Prompt library</span>
          </button>

          <button className="nav-item" onClick={openPromptOverrides}>
            <Icon name="sparkles" /><span className="nav-label">Prompt overrides</span>
          </button>

          <button className="nav-item" onClick={() => setScreenshotTarget({ target: null, sessionId: null })}><Icon name="file" /><span className="nav-label">Validation screenshots</span></button>

          <button className="nav-item" onClick={() => setShowDormancy(true)}>
            <Icon name="zap" /><span className="nav-label">Dormant features</span>
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

          <SidebarTree
            projects={projects}
            sidebar={workspace.data?.sidebar}
            stoppedSessionIds={workspace.data?.stopped_session_ids ?? []}
            selection={view?.kind === "project" ? view
              : view?.kind === "feature" ? { ...view, tab: tabByFeature[view.featureId] } : null}
            deletingFeatureId={deleteFeatureMutation.isPending
              ? deleteFeatureMutation.variables?.target.feature_id ?? null : null}
            pausedPlan={activePlan && planMinimized ? {
              featureId: activePlan.pending_project_name ? null : activePlan.interview_key,
              projectName: activePlan.pending_project_name ?? null,
              featureName: activePlan.feature_name,
            } : null}
            onSelectProject={(projectId) => setView({ kind: "project", projectId })}
            onSelectFeature={(projectId, featureId) => setView({ kind: "feature", projectId, featureId })}
            onSelectSession={(target, kind) => {
              setView({ kind: "feature", projectId: target.project_id, featureId: target.feature_id });
              setTabByFeature((current) => ({
                ...current, [target.feature_id]: kind === "todos" ? TODOS_TAB : target.session_id,
              }));
            }}
            onToggleCollapsed={toggleCollapsed}
            onResumePlan={() => setPlanMinimized(false)}
            onCreateFeature={setCreateFeatureFor}
            renderFeatureExtra={(feature) => pendingEdits[feature.id] > 0 && (
              <span className="nav-count nav-count-attention" title="Edits waiting for review">
                {pendingEdits[feature.id]}
              </span>
            )}
          />
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

      {supervisedTarget && <SupervisedEditsPanel key={`${supervisedTarget.project_id}:${supervisedTarget.feature_id}`}
        ref={supervisedPanel} target={supervisedTarget} onClose={() => setSupervisedTarget(null)} />}
      {showDormancy && <DormancyPanel onClose={() => setShowDormancy(false)}
        onOpenFeature={(target) => {
          setShowDormancy(false);
          setView({ kind: "feature", projectId: target.project_id, featureId: target.feature_id });
        }} />}
      {screenshotTarget && <ScreenshotsPanel target={screenshotTarget.target} sessionId={screenshotTarget.sessionId} onClose={() => setScreenshotTarget(null)} />}
      {prTriageTarget && <PrTriagePanel key={`${prTriageTarget.project_id}:${prTriageTarget.feature_id}`} target={prTriageTarget} onClose={() => setPrTriageTarget(null)} />}
      {diffTarget && <DiffPanel key={`${diffTarget.project_id}:${diffTarget.feature_id}`} target={diffTarget} onClose={() => setDiffTarget(null)} />}
      {review && <ReviewPanel key={review.workflow_id} view={review} busy={reviewBusy} error={reviewError} onAct={actReview} onEditPrompt={() => void editPrecallPrompt()} onSyntaxInstalled={() => void refreshReviewSyntax()} />}
      {learning && (
        <LearningPanel key={learning.workflow_id} view={learning} busy={learningBusy || learningApproval !== null}
          onAct={actLearning} onLaunch={(qaId) => void launchLearning(qaId)}
          onClose={() => void actLearning({ kind: "close" })} />
      )}
      {learningApproval && (
        <ApprovalDialog label="Approve Learning agent" title="Start editing agent?"
          message={learningApproval.message} confirmLabel="Start anyway" busy={learningBusy}
          onConfirm={() => void launchLearning(learningApproval.qaId, true)}
          onCancel={() => setLearningApproval(null)} />
      )}

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
            pendingSessionId={pendingSessionByFeature[selectedFeature.id]}
            onTab={(tab) => setTabByFeature((current) => ({ ...current, [selectedFeature.id]: tab }))}
            onBack={() => setView({ kind: "project", projectId: selectedProject.id })}
            onPlan={(quick) => void beginPlan(
              { project_id: selectedProject.id, feature_id: selectedFeature.id },
              quick,
            )}
            onLearning={() => void beginLearning({ project_id: selectedProject.id, feature_id: selectedFeature.id })}
            learningBusy={learningBusy}
            onDiff={() => setDiffTarget({ project_id: selectedProject.id, feature_id: selectedFeature.id })}
            pendingEdits={pendingEdits[selectedFeature.id] ?? 0}
            onSupervisedEdits={() => openSupervisedEdits({ project_id: selectedProject.id, feature_id: selectedFeature.id })}
            onScreenshots={(sessionId) => setScreenshotTarget({ target: { project_id: selectedProject.id, feature_id: selectedFeature.id }, sessionId })}
            onPrTriage={() => setPrTriageTarget({ project_id: selectedProject.id, feature_id: selectedFeature.id })}
            onReview={() => void beginReview({ project_id: selectedProject.id, feature_id: selectedFeature.id })}
            reviewBusy={reviewBusy}
            onNewSession={() => void openNewSession(selectedProject, selectedFeature)}
            newSessionLoading={newSessionLoading}
            stoppedSessionIds={workspace.data?.stopped_session_ids ?? []}
            sessionLifecycle={sessionLifecycle(selectedProject.id, selectedFeature)}
            onCloseSession={(session) => setCloseSessionDialog({
              target: {
                project_id: selectedProject.id,
                feature_id: selectedFeature.id,
                session_id: session.id,
              },
              label: session.label,
              last: closingStopsFeature(
                selectedFeature,
                session.id,
                workspace.data?.stopped_session_ids ?? [],
              ),
            })}
            onDeleteFeature={() => setDeleteFeatureDialog({
              target: { project_id: selectedProject.id, feature_id: selectedFeature.id },
              projectName: selectedProject.name,
              featureName: selectedFeature.name,
              isWorktree: selectedFeature.is_worktree,
              unfinished: null,
            })}
            {...lifecycle(selectedProject.id, selectedFeature)}
            onPromptLibrary={openPromptLibrary}
            drafts={drafts}
            onDraftChange={changeDraft}
            sendingPrompts={sendingPrompts}
            onSendDraft={(target, text) => void sendDraft(target, text)}
            composerFocusKey={composerFocusKey}
            onComposerFocusHandled={composerFocusHandled}
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
          projectId={createFeatureProject.id}
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
              onEditPrompt={() => void editPrecallPrompt()}
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

      {pendingSessionStart && (
        <ApprovalDialog
          label="Approve session start"
          title="Start another agent?"
          message={pendingSessionStart.message}
          confirmLabel="Start anyway"
          busy={startSessionMutation.isPending}
          onConfirm={() => startSessionMutation.mutate({ target: pendingSessionStart.target, approved: true })}
          onCancel={() => setPendingSessionStart(null)}
        />
      )}

      {recoveryDialog && (
        <RecoveryDialog
          option={recoveryDialog.option}
          sessions={recoveryDialog.sessions}
          selectedId={recoveryDialog.selectedId}
          loading={recoveryDialog.loading}
          busy={recoverSessionMutation.isPending}
          onChoose={() => void loadRecoverySessions()}
          onBack={() => setRecoveryDialog((current) => current && { ...current, sessions: null })}
          onSelect={(id) => setRecoveryDialog((current) => current && { ...current, selectedId: id })}
          onRecover={(choice, pickedId) => recoverSessionMutation.mutate({
            target: recoveryDialog.target,
            choice,
            pickedId,
            approved: false,
          })}
          onClose={() => setRecoveryDialog(null)}
        />
      )}

      {newSessionDialog && (
        <NewSessionDialog
          options={newSessionDialog.options}
          preferredKind={newSessionDialog.preferredKind}
          busy={addSessionMutation.isPending}
          onCreate={(kind, label) => requestAddSession({
            target: newSessionDialog.target,
            kind,
            label,
            approved: false,
          })}
          onClose={() => setNewSessionDialog(null)}
        />
      )}

      {closeSessionDialog && (
        <ApprovalDialog
          label="Close session"
          title={`Close ${closeSessionDialog.label}?`}
          message={closeSessionDialog.last
            ? "This kills its tmux window and removes the session. It is the feature's last running session, so the feature stops too."
            : "This kills its tmux window and removes the session."}
          confirmLabel="Close session"
          busy={removeSessionMutation.isPending}
          onConfirm={() => requestRemoveSession(closeSessionDialog.target)}
          onCancel={() => setCloseSessionDialog(null)}
        />
      )}

      {deleteFeatureDialog && (
        <DeleteFeatureDialog
          projectName={deleteFeatureDialog.projectName}
          featureName={deleteFeatureDialog.featureName}
          isWorktree={deleteFeatureDialog.isWorktree}
          unfinished={deleteFeatureDialog.unfinished}
          todoHost={deleteFeatureDialog.todoHost}
          busy={deleteFeatureMutation.isPending}
          onConfirm={(todos, todoHost) => requestDeleteFeature(deleteFeatureDialog.target, todos, todoHost)}
          onClose={() => setDeleteFeatureDialog(null)}
        />
      )}

      {pendingAddSessionApproval && (
        <ApprovalDialog
          label="Approve new session"
          title="Start another agent?"
          message={pendingAddSessionApproval.message}
          confirmLabel="Start anyway"
          busy={addSessionMutation.isPending}
          onConfirm={() => requestAddSession({
            target: pendingAddSessionApproval.target,
            kind: pendingAddSessionApproval.kind,
            label: pendingAddSessionApproval.label,
            approved: true,
          })}
          onCancel={() => setPendingAddSessionApproval(null)}
        />
      )}

      {pendingRecoveryApproval && (
        <ApprovalDialog
          label="Approve agent recovery"
          title="Start another agent?"
          message={pendingRecoveryApproval.message}
          confirmLabel="Start anyway"
          busy={recoverSessionMutation.isPending}
          onConfirm={() => recoverSessionMutation.mutate({
            target: pendingRecoveryApproval.target,
            choice: pendingRecoveryApproval.choice,
            pickedId: pendingRecoveryApproval.pickedId,
            approved: true,
          })}
          onCancel={() => setPendingRecoveryApproval(null)}
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

      {promptLibrary && <PromptLibraryPanel
        initialScope={promptLibrary.scope} initialTarget={promptLibrary.target} projects={projects}
        onClose={() => setPromptLibrary(null)} onInsert={insertLibraryPrompt}
      />}

      {promptOverrides && <PromptOverridesPanel
        initialContext={promptOverrides.context} initialPromptId={promptOverrides.promptId}
        initialHarness={promptOverrides.harness} fromPrecall={promptOverrides.fromPrecall}
        contextNote={promptOverrides.contextNote ?? null} projects={projects} onClose={() => setPromptOverrides(null)}
      />}

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
                        {stoppedSessionCount(feature) > 0 && (
                          <>
                            <span className="dot-sep" />
                            {stoppedSessionCount(feature)} stopped
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
  pendingSessionId,
  onTab,
  onBack,
  onPlan,
  onLearning,
  learningBusy,
  onDiff,
  pendingEdits,
  onSupervisedEdits,
  onPrTriage,
  onScreenshots,
  onReview,
  reviewBusy,
  onNewSession,
  newSessionLoading,
  stoppedSessionIds,
  sessionLifecycle,
  onCloseSession,
  onDeleteFeature,
  starting,
  stopping,
  onStart,
  onStop,
  drafts,
  onPromptLibrary,
  onDraftChange,
  sendingPrompts,
  onSendDraft,
  composerFocusKey,
  onComposerFocusHandled,
  todoPanel,
}: Lifecycle & {
  project: Project;
  feature: Feature;
  harnessName: (slug: AgentSlug) => string;
  modeName: (slug: ModeSlug) => string;
  tab: string | undefined;
  /** A session this page just launched that the snapshot may not have yet. */
  pendingSessionId: string | undefined;
  onTab: (tab: string) => void;
  onBack: () => void;
  onPlan: (quick: boolean) => void;
  onLearning: () => void;
  learningBusy: boolean;
  onDiff: () => void;
  /** Supervised edits waiting for an answer in this feature. */
  pendingEdits: number;
  onSupervisedEdits: () => void;
  onPrTriage: () => void;
  onScreenshots: (sessionId: string | null) => void;
  onReview: () => void;
  reviewBusy: boolean;
  onNewSession: () => void;
  newSessionLoading: boolean;
  stoppedSessionIds: string[];
  /** Start/stop for one session, leaving the rest of the feature alone. */
  sessionLifecycle: (session: FeatureSession) => Lifecycle;
  onCloseSession: (session: FeatureSession) => void;
  onDeleteFeature: () => void;
  drafts: Record<string, string>;
  onPromptLibrary: (target: SessionTarget) => void;
  onDraftChange: (target: SessionTarget, text: string) => void;
  sendingPrompts: Record<string, boolean>;
  onSendDraft: (target: SessionTarget, text: string) => void;
  /** The session whose composer a handoff just seeded and should focus. */
  composerFocusKey: string | null;
  onComposerFocusHandled: () => void;
  todoPanel: ReactNode;
}) {
  const sessions = feature.sessions.filter((session) => session.kind !== "todos");
  const known = (id: string | undefined) =>
    id === TODOS_TAB || sessions.some((session) => session.id === id);
  // A remembered tab whose session is gone falls back to the first one,
  // unless it is the session a launch just created and the snapshot hasn't
  // caught up yet -- keep that tab so the handoff lands somewhere.
  const pendingSession = tab !== undefined && !known(tab) && tab === pendingSessionId;
  const activeTab = tab !== undefined && (known(tab) || pendingSession)
    ? tab
    : sessions[0]?.id ?? TODOS_TAB;
  const isStopped = feature.status === "stopped";
  const target: SessionTarget | null = activeTab === TODOS_TAB ? null : {
    project_id: project.id,
    feature_id: feature.id,
    session_id: activeTab,
  };
  const activeKey = target ? sessionKey(target) : null;
  const [connectedKey, setConnectedKey] = useState<string | null>(null);
  const onTerminalReady = useCallback((ready: boolean) => {
    setConnectedKey(ready ? activeKey : null);
  }, [activeKey]);
  const isRunning = (session: FeatureSession) => sessionRunning(feature, session, stoppedSessionIds);
  const activeSession = sessions.find((session) => session.id === activeTab);
  const activeSessionRunning = activeSession !== undefined && isRunning(activeSession);
  const isAgent = activeSession !== undefined &&
    ["claude", "codex", "opencode", "pi"].includes(activeSession.kind);
  const activeLifecycle = activeSession && sessionLifecycle(activeSession);
  const composer = target && isAgent ? (
    <PromptComposer key={`composer:${sessionKey(target)}`}
      text={drafts[sessionKey(target)] ?? ""} sending={sendingPrompts[sessionKey(target)] ?? false}
      ready={activeSessionRunning && !isStopped && connectedKey === sessionKey(target)}
      connectionMessage={isStopped || !activeSessionRunning ? "Start this session to send your draft." : undefined}
      focusRequested={composerFocusKey === sessionKey(target)} onFocusHandled={onComposerFocusHandled}
      onChange={(text) => onDraftChange(target, text)} onClear={() => onDraftChange(target, "")}
      onSend={() => onSendDraft(target, drafts[sessionKey(target)] ?? "")}
      onLibrary={() => onPromptLibrary(target)} />
  ) : null;

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
            <button className="btn btn-secondary" onClick={onNewSession} disabled={newSessionLoading}>
              {newSessionLoading ? <Spinner /> : <Icon name="plus" size={12} />} New session
            </button>
            <button className="btn btn-secondary" onClick={onLearning} disabled={learningBusy}>
              <Icon name="file" size={12} /> Learning
            </button>
            {project.is_git && <button className="btn btn-secondary" onClick={onDiff}>
              <Icon name="branch" size={12} /> Changes
            </button>}
            {pendingEdits > 0 && <button className="btn btn-secondary" onClick={onSupervisedEdits}>
              <Icon name="check" size={12} /> Supervised edits
              {pendingEdits > 0 && <span className="nav-count nav-count-attention">{pendingEdits}</span>}
            </button>}
            {project.is_git && <button className="btn btn-secondary" onClick={onReview} disabled={reviewBusy}>
              {reviewBusy ? <Spinner /> : <Icon name="file" size={12} />} Final Review
            </button>}
            <button className="btn btn-secondary" onClick={() => onScreenshots(null)}>Screenshots</button>
            {activeSession && <button className="btn btn-secondary" onClick={() => onScreenshots(activeSession.id)}>Session screenshots</button>}
            {project.is_git && <button className="btn btn-secondary" onClick={onPrTriage}>
              <Icon name="inbox" size={12} /> PR Triage
            </button>}
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
            {activeSession && !isStopped && (
              <SessionStartStopButton
                label={activeSession.label}
                running={activeSessionRunning}
                {...sessionLifecycle(activeSession)}
              />
            )}
            <StartStopButton
              status={feature.status}
              starting={starting}
              stopping={stopping}
              onStart={onStart}
              onStop={onStop}
            />
            <Menu
              label="More feature actions"
              icon="more"
              className="btn btn-secondary btn-icon"
              items={[
                ...(feature.mode === "vibeless" ? [{ label: "Supervised edits", icon: "check" as const, onSelect: onSupervisedEdits }] : []),
                { label: "Delete feature", icon: "trash", danger: true, onSelect: onDeleteFeature },
              ]}
            />
          </>
        }
      />

      <div className="tabs" role="tablist" aria-label="Feature sessions">
        {sessions.map((session) => (
          <span key={session.id} className="tab-group">
            <button
              role="tab"
              aria-selected={activeTab === session.id}
              className={[
                "tab",
                activeTab === session.id && "tab-active",
                !isStopped && !isRunning(session) && "tab-stopped",
              ].filter(Boolean).join(" ")}
              onClick={() => onTab(session.id)}
              title={`${session.kind} — ${isRunning(session) ? "running" : "stopped"}`}
            >
              <SessionStateDot running={isRunning(session)} />
              {session.label}
            </button>
            <span className="tab-close">
              <Menu
                label={`${session.label} actions`}
                className="btn btn-ghost btn-icon btn-sm"
                items={[
                  isRunning(session)
                    ? { label: "Stop session", icon: "stop", onSelect: sessionLifecycle(session).onStop }
                    : {
                      label: "Start session",
                      icon: "play",
                      disabled: sessionLifecycle(session).starting || starting,
                      hint: isStopped ? "Starts the feature" : undefined,
                      onSelect: sessionLifecycle(session).onStart,
                    },
                  { label: "Close session", icon: "x", danger: true, onSelect: () => onCloseSession(session) },
                ]}
              />
            </span>
          </span>
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
              <button className="btn btn-primary"
                onClick={activeLifecycle ? activeLifecycle.onStart : onStart}
                disabled={starting || activeLifecycle?.starting}>
                {starting || activeLifecycle?.starting ? <Spinner /> : <Icon name="play" size={12} />}
                {starting || activeLifecycle?.starting ? "Starting…" : "Start feature"}
              </button>
            }
          >
            Starting it from this tab offers to resume this session's saved conversation, if it has one.
          </EmptyState>
        )}
        {target && !isStopped && activeSession && activeLifecycle && !activeSessionRunning && (
          <EmptyState
            icon="terminal"
            title={`${activeSession.label} is stopped`}
            action={
              <SessionStartStopButton
                label={activeSession.label}
                running={false}
                {...activeLifecycle}
              />
            }
          >
            {activeSession.stopped
              ? "It stays stopped when the feature starts, until you start it here. "
              : "The rest of the feature is still running. "}
            Starting an agent session offers to resume its saved conversation when there is one.
          </EmptyState>
        )}
        {target && isAgent && (isStopped || !activeSessionRunning) && (
          <div className="stopped-composer">{composer}</div>
        )}
        {target && !isStopped && (activeSessionRunning || !activeSession) && (
          <div className="session">
            <TerminalPane key={sessionKey(target)} target={target} onReadyChange={onTerminalReady} />
            {composer}
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

export function CreateFeatureForm({
  projectId,
  projectName,
  agents,
  modes,
  isGit,
  defaultUseWorktree,
  pending,
  onCancel,
  onSubmit,
}: {
  projectId: string;
  projectName: string;
  agents: HarnessInfo[];
  modes: ModeInfo[];
  isGit: boolean;
  defaultUseWorktree: boolean;
  pending: boolean;
  onCancel: () => void;
  onSubmit: (request: Omit<CreateFeatureRequest, "project_name">, planKind: PlanKind) => void;
}) {
  const [branch, setBranch] = useState("");
  const [agent, setAgent] = useState<AgentSlug>(agents[0]?.slug ?? "claude");
  const [mode, setMode] = useState<ModeSlug>(modes[0]?.slug ?? "vibe");
  const [useWorktree, setUseWorktree] = useState(defaultUseWorktree);
  const [planKind, setPlanKind] = useState<PlanKind>("none");
  const modeInfo = modes.find((candidate) => candidate.slug === mode);
  const hook = useWorktreeHookChoice(projectId, isGit && useWorktree);

  return (
    <Modal
      label="New feature"
      title="New feature"
      subtitle={`in ${projectName}`}
      onClose={onCancel}
      dismissable={!pending}
      onSubmit={() => {
        if (pending || !hook.ready || !branch.trim()) return;
        onSubmit({
          branch: branch.trim(),
          agent,
          mode,
          review: false,
          plan_mode: planKind !== "none",
          create_terminal: false,
          use_worktree: useWorktree,
          enable_chrome: false,
          hook_choice: hook.choice || null,
          dry_run: false,
        }, planKind);
      }}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onCancel} disabled={pending}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={pending || !hook.ready || !branch.trim()}>
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
        <WorktreeHookField hook={hook} disabled={pending} />
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
  const hook = useWorktreeHookChoice(project?.id, !!project);

  return (
    <Modal
      label={kind === "plan" ? "Plan TODO in a new feature" : "Start TODO in a new feature"}
      title={kind === "plan" ? "Plan in a new feature" : "Start in a new feature"}
      subtitle={todoTitle}
      onClose={onCancel}
      dismissable={!pending}
      onSubmit={() => {
        if (pending || !project || !branch.trim() || !hook.ready) return;
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
          hook_choice: hook.choice || null,
          dry_run: false,
        });
      }}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onCancel} disabled={pending}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={pending || !project || !branch.trim() || !hook.ready}>
            {pending && <Spinner />}
            {kind === "plan" ? "Start plan" : "Create and start"}
          </button>
        </>
      }
    >
      <p className="muted small modal-lead">
        {kind === "plan"
          ? "Creates the worktree and runs its setup before the interview. The agent starts after you accept the plan."
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
        <WorktreeHookField hook={hook} disabled={pending} />
      </div>
    </Modal>
  );
}
