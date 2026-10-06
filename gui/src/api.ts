import { invoke } from "@tauri-apps/api/core";

// Mirrors `gui_contract`/`automation`'s Rust types (src/gui_contract.rs,
// src/automation.rs). Kept in one place so the rest of the frontend imports
// types instead of re-declaring them per component.

export type AgentSlug = "claude" | "opencode" | "codex" | "pi";
export type ModeSlug = "vibeless" | "vibe" | "supervibe";
export type FeatureStatus = "active" | "idle" | "stopped";

export interface HarnessInfo {
  slug: AgentSlug;
  display_name: string;
}

export interface ModeInfo {
  slug: ModeSlug;
  display_name: string;
  description: string;
}

export interface FeatureSession {
  id: string;
  kind: string;
  label: string;
  tmux_window: string;
  /** Stopped on its own while its feature runs; omitted when false. */
  stopped?: boolean;
}

export interface IssueSource {
  host: string;
  owner: string;
  repository: string;
  number: number;
}

// Persisted fields the TUI dashboard tree reads are optional here so older
// fixtures and partial snapshots stay valid; the Rust store always sends them.
export interface Feature {
  id: string;
  name: string;
  branch: string;
  workdir: string;
  is_worktree: boolean;
  status: FeatureStatus;
  agent: AgentSlug;
  mode: ModeSlug;
  sessions: FeatureSession[];
  /** Shared with the TUI tree; Rust defaults it to collapsed. */
  collapsed?: boolean;
  nickname?: string | null;
  review?: boolean;
  plan_mode?: boolean;
  remote_control?: boolean;
  pending_worktree_script?: boolean;
  ready?: boolean;
  created_at?: string;
  summary?: string | null;
  summary_updated_at?: string | null;
  issue_source?: IssueSource | null;
}

export interface Project {
  id: string;
  name: string;
  repo: string;
  is_git: boolean;
  features: Feature[];
  /** Shared with the TUI tree. */
  collapsed?: boolean;
}

/** What the TUI tree derives at render time (`gui_contract::sidebar`). */
export interface SidebarSnapshot {
  projects: Record<string, { repo_display: string }>;
  features: Record<string, SidebarFeature>;
  sessions: Record<string, SidebarSession>;
}

export type SidebarPr =
  | { state: "open"; number: number; unresolved_threads: number | null }
  | { state: "merged" | "closed"; number: number };

export interface SidebarFeature {
  workdir_display: string;
  created_age: string;
  issue: string | null;
  summary_age: string | null;
  usage: string | null;
  pr: SidebarPr | null;
  thinking: boolean;
  waiting_for_input: boolean;
  pending_input: boolean;
}

export interface SidebarContext {
  text: string;
  band: "normal" | "warning" | "critical";
  stale: boolean;
  pending_reset: boolean;
}

export interface SidebarSession {
  status_text: string | null;
  context: SidebarContext | null;
  icon: string | null;
  icon_nerd: string | null;
}

export interface WorkspaceSnapshot {
  projects: Project[];
  snapshot_at: string;
  /** Sessions of a running feature whose tmux window is gone. */
  stopped_session_ids: string[];
  /** Absent only in fixtures written before sidebar parity. */
  sidebar?: SidebarSnapshot;
}

export interface CollapseTarget {
  project_id: string;
  feature_id: string | null;
}

/** Persist a tree row's collapse state in the store the TUI shares. */
export function setCollapsed(target: CollapseTarget, collapsed: boolean): Promise<WorkspaceSnapshot> {
  return invoke("set_collapsed", { target, collapsed });
}

export type GuiErrorKind = "not_found" | "conflict" | "needs_approval" | "internal";

export interface GuiError {
  kind: GuiErrorKind;
  message: string;
}

/** Type guard: Tauri command rejections are the exact `GuiError` JSON value
 * (`kind`/`message`), not a JS `Error`, since the backend commands return
 * `Result<T, GuiError>`. Anything else (a transport-level failure) falls
 * back to `internal` with the raw stringified value. */
export function asGuiError(err: unknown): GuiError {
  if (
    typeof err === "object" &&
    err !== null &&
    "kind" in err &&
    "message" in err
  ) {
    return err as GuiError;
  }
  return { kind: "internal", message: String(err) };
}

export function supportedHarnesses(): Promise<HarnessInfo[]> {
  return invoke("supported_harnesses");
}

export function supportedModes(): Promise<ModeInfo[]> {
  return invoke("supported_modes");
}

export function getSnapshot(): Promise<WorkspaceSnapshot> {
  return invoke("get_snapshot");
}

export interface CreateProjectRequest {
  path: string;
  project_name: string;
  preferred_agent: AgentSlug | null;
  dry_run: boolean;
}

export interface CreateProjectResponse {
  ok: boolean;
  project_id: string | null;
  project_path: string;
  message: string;
}

export function createProject(
  request: CreateProjectRequest,
): Promise<CreateProjectResponse> {
  return invoke("create_project", { request });
}

export interface CreateFeatureRequest {
  project_name: string;
  branch: string;
  agent: AgentSlug;
  mode: ModeSlug;
  review: boolean;
  plan_mode: boolean;
  create_terminal: boolean;
  use_worktree: boolean | null;
  enable_chrome: boolean;
  hook_choice: string | null;
  dry_run: boolean;
}

export interface CreateFeatureResponse {
  ok: boolean;
  project_id: string | null;
  feature_id: string | null;
  started: boolean;
  message: string;
}

export interface WorktreeHookPrompt {
  title: string;
  options: string[];
}

export function worktreeHookPrompt(projectId: string): Promise<WorktreeHookPrompt | null> {
  return invoke("worktree_hook_prompt", { projectId });
}

export interface FeatureTarget {
  project_id: string;
  feature_id: string;
}

export interface SessionTarget extends FeatureTarget {
  session_id: string;
}

export interface StartFeatureResponse {
  feature_id: string;
  already_running: boolean;
  message: string;
}

export interface StopFeatureResponse {
  feature_id: string;
  already_stopped: boolean;
  message: string;
}

export interface StartSessionResponse {
  session_id: string;
  already_running: boolean;
  message: string;
}

export interface StopSessionResponse {
  session_id: string;
  already_stopped: boolean;
  /** It was the feature's last running session, so the feature stopped too. */
  feature_stopped: boolean;
  message: string;
}

export type NewSessionKind = AgentSlug | "terminal" | "nvim";

export interface NewSessionOption {
  kind: NewSessionKind;
  label: string;
}

export interface AddSessionResponse {
  target: SessionTarget;
  label: string;
}

export function newSessionOptions(target: FeatureTarget): Promise<NewSessionOption[]> {
  return invoke("new_session_options", { target });
}

export function addSession(
  target: FeatureTarget,
  kind: NewSessionKind,
  label: string | null,
  approved: boolean,
): Promise<AddSessionResponse> {
  return invoke("add_session", { target, kind, label, approved });
}

export interface RemoveSessionResponse {
  session_id: string;
  /** Removing a feature's last running session stops the feature. */
  feature_stopped: boolean;
  message: string;
}

export function removeSession(target: SessionTarget): Promise<RemoveSessionResponse> {
  return invoke("remove_session", { target });
}

/** What happens to a deleted worktree's unfinished TODOs. */
export type TodoDeleteChoice = "move_to_project" | "move_to_global" | "delete";

export interface TodoHostPrompt {
  list_id: string;
  todo_count: number;
  candidates: { feature_id: string; name: string }[];
}

export interface TodoHostChoice {
  list_id: string;
  /** Null explicitly deletes the project TODO list. */
  feature_id: string | null;
}

export type DeleteFeatureResponse =
  | { status: "deleted"; feature_id: string; message: string }
  /** Nothing was touched; resend with a `TodoDeleteChoice`. */
  | { status: "needs_todo_disposition"; unfinished: number }
  | { status: "needs_todo_host"; prompt: TodoHostPrompt };

export function deleteFeature(
  target: FeatureTarget,
  todos: TodoDeleteChoice | null,
  todoHost: TodoHostChoice | null,
): Promise<DeleteFeatureResponse> {
  return invoke("delete_feature", { target, todos, todoHost });
}

export interface SessionRecoveryOption {
  harness: string;
  saved_id: string;
}

export interface SavedAgentSession {
  id: string;
  title: string;
  updated: number;
}

export type SessionRecoveryChoice = "resume" | "clear" | "pick";

export function sessionRecoveryOption(target: SessionTarget): Promise<SessionRecoveryOption | null> {
  return invoke("session_recovery_option", { target });
}

export function savedAgentSessions(target: SessionTarget): Promise<SavedAgentSession[]> {
  return invoke("saved_agent_sessions", { target });
}

export function recoverSession(
  target: SessionTarget,
  choice: SessionRecoveryChoice,
  pickedId: string | null,
  approved: boolean,
): Promise<StartFeatureResponse> {
  return invoke("recover_session", { target, choice, pickedId, approved });
}

export function createFeature(
  request: CreateFeatureRequest,
): Promise<CreateFeatureResponse> {
  return invoke("create_feature", { request });
}

export function startFeature(
  target: FeatureTarget,
  approved = false,
): Promise<StartFeatureResponse> {
  return invoke("start_feature", { target, approved });
}

export function stopFeature(
  target: FeatureTarget,
): Promise<StopFeatureResponse> {
  return invoke("stop_feature", { target });
}

export function startSession(
  target: SessionTarget,
  approved = false,
): Promise<StartSessionResponse> {
  return invoke("start_session", { target, approved });
}

export function stopSession(target: SessionTarget): Promise<StopSessionResponse> {
  return invoke("stop_session", { target });
}

// Mirrors `gui_todos`'s Rust types (src/gui_todos.rs, src/db/todos.rs).

export type TodoStatus = "not_started" | "in_progress" | "completed";
export type TodoPriority = "high" | "med" | "low";

// Internally-tagged (`#[serde(tag = "kind")]`) to match `TodoScope`/
// `TodoScopeRequest` on the Rust side exactly, including `Worktree`'s
// newtype variant flattening `FeatureTarget`'s fields alongside the tag.
export type TodoScopeRequest =
  | ({ kind: "worktree" } & FeatureTarget)
  | { kind: "project"; project_id: string }
  | { kind: "global" };

export type TodoScope =
  | { kind: "worktree"; project_id: string; workdir: string }
  | { kind: "project"; project_id: string }
  | { kind: "global" };

export interface Todo {
  id: string;
  list_id: string;
  title: string;
  body: string | null;
  priority: TodoPriority;
  sort_order: number;
  work: { status: TodoStatus; agent_session_id: string | null };
  linked_feature_id: string | null;
  created_at: string;
  updated_at: string;
}

export interface TodoList {
  id: string;
  scope: TodoScope;
  feature_id: string | null;
  carry_over: string | null;
  created_at: string;
  updated_at: string;
}

export interface TodoListView {
  list: TodoList;
  todos: Todo[];
}

export function todoList(request: TodoScopeRequest): Promise<TodoListView | null> {
  return invoke("todo_list", { request });
}

export function todoAdd(
  request: TodoScopeRequest,
  title: string,
  options?: { hostFeatureId?: string; body?: string; priority?: TodoPriority },
): Promise<TodoListView> {
  return invoke("todo_add", {
    request,
    hostFeatureId: options?.hostFeatureId ?? null,
    title,
    body: options?.body ?? null,
    priority: options?.priority ?? "med",
  });
}

export function todoSetStatus(todoId: string, status: TodoStatus): Promise<void> {
  return invoke("todo_set_status", { todoId, status });
}

export function todoDelete(todoId: string): Promise<void> {
  return invoke("todo_delete", { todoId });
}

export function todoMove(
  todoId: string,
  target: TodoScopeRequest,
): Promise<TodoListView> {
  return invoke("todo_move", { todoId, target });
}

export function todoCopy(
  todoId: string,
  target: TodoScopeRequest,
): Promise<TodoListView> {
  return invoke("todo_copy", { todoId, target });
}

export function todoReorder(orderedIds: string[]): Promise<void> {
  return invoke("todo_reorder", { orderedIds });
}

export interface TodoAgentLaunchResponse {
  target: SessionTarget;
  draft_prompt: string;
  reused_session: boolean;
}

export function todoLaunchAgent(
  todoId: string,
  target: FeatureTarget,
  approved = false,
): Promise<TodoAgentLaunchResponse> {
  return invoke("todo_launch_agent", { todoId, target, approved });
}

export function todoLaunchNewFeature(
  todoId: string,
  request: CreateFeatureRequest,
  approved = false,
): Promise<TodoAgentLaunchResponse> {
  return invoke("todo_launch_new_feature", { todoId, request, approved });
}

export function terminalSubmitPrompt(target: SessionTarget, text: string): Promise<void> {
  return invoke("terminal_submit_prompt", {
    key: `${target.feature_id}:${target.session_id}`,
    text,
  });
}

export interface PlanQuestionView {
  id: string;
  text: string;
  optional: boolean;
  options: string[] | null;
}

export interface PlanView {
  interview_key: string;
  feature_name: string;
  /** Creation-time interviews only: the project the new feature will join. */
  pending_project_name?: string | null;
  kind: "full" | "quick";
  phase: string;
  step_key: string;
  question_index: number;
  question_count: number;
  question: PlanQuestionView | null;
  editor_text: string;
  selected_option: number | null;
  review_markdown: string | null;
  critique: string | null;
  attached_docs: string[];
  kickoff_target: string | null;
}

export interface PlanStatus {
  active: PlanView | null;
  precall: PrecallView | null;
  message: string | null;
  hook_warning: string | null;
  handoff: PlanHandoff | null;
}

export interface PlanHandoff {
  target: SessionTarget;
  draft_prompt: string;
}

export interface PrecallView {
  title: string;
  harness: string;
  preview: string;
  viewing: boolean;
}

export interface PlanInput {
  text: string;
  selected_option: number | null;
}

export type PlanAction =
  | "resume" | "discard_draft" | "next" | "back" | "skip"
  | "finish_early" | "opt_in_ai" | "begin_edit" | "save_edit"
  | "cancel_edit" | "regenerate" | "request_critique"
  | "close_critique" | "revise_from_critique" | "begin_feedback"
  | "submit_feedback" | "cancel_feedback" | "begin_investigation"
  | "submit_investigation" | "cancel_investigation" | "restore_prior"
  | "attach_doc" | "remove_doc" | "accept" | "accept_approved" | "cancel"
  | "kickoff_accept" | "kickoff_decline"
  | "precall_confirm" | "precall_cancel" | "precall_toggle_view";

export function planBegin(target: FeatureTarget, quick = false): Promise<PlanStatus> {
  return invoke("plan_begin", { target, quick });
}

export function planBeginTodoHost(todoId: string, target: FeatureTarget): Promise<PlanStatus> {
  return invoke("plan_begin_todo_host", { todoId, target });
}

export function planBeginCreation(request: CreateFeatureRequest, quick: boolean): Promise<PlanStatus> {
  return invoke("plan_begin_creation", { request, quick });
}

export function planBeginTodoNew(todoId: string, request: CreateFeatureRequest): Promise<PlanStatus> {
  return invoke("plan_begin_todo_new", { todoId, request });
}

export function planSnapshot(): Promise<PlanStatus> {
  return invoke("plan_snapshot");
}

export function planAct(
  expectedStep: string,
  action: PlanAction,
  input: PlanInput | null = null,
): Promise<PlanStatus> {
  return invoke("plan_act", { expectedStep, action, input });
}

export interface LearningEntry {
  key: string;
  label: string;
  kind: "header" | "project" | "dir" | "file";
  depth: number;
  expanded: boolean;
}
export interface LearningAnswer {
  id: string;
  parent_id: string | null;
  question: string;
  answer: string | null;
  anchor: string;
  status: "pending" | "running" | "answered" | "failed";
  intent: "explain" | "action";
  run_mode: string;
  harness: AgentSlug;
  error: string | null;
  drift: string | null;
  spawned_session_id: string | null;
  /** The TODO this answer was kept as, while that item still exists. */
  todo_id: string | null;
  todo_seed: { title: string; notes: string } | null;
}
export interface LearningStarter { text: string; intent: "explain" | "action" }
export interface LearningHunk { index: number; start: number; end: number }
export interface LearningView {
  workflow_id: string;
  revision: number;
  target: FeatureTarget;
  feature_name: string;
  scope: "repo_tree" | "branch_changes";
  is_git: boolean;
  entries: LearningEntry[];
  content_path: string | null;
  content: string[];
  content_line_labels: string[];
  content_error: string | null;
  anchor: string;
  /** 1-based inclusive rows of `content` under a line or hunk anchor. */
  selection: [number, number] | null;
  hunks: LearningHunk[];
  starters: LearningStarter[];
  can_keep_todo: boolean;
  harness: AgentSlug;
  harnesses: AgentSlug[];
  level: "newcomer" | "familiar";
  history_saved: boolean;
  qa: LearningAnswer[];
  error: string | null;
  notice: string | null;
}
export type LearningAction =
  | { kind: "select_entry"; key: string }
  | { kind: "toggle_scope" | "refresh" | "project_anchor" | "file_anchor" | "close" }
  | { kind: "lines_anchor"; start: number; end: number }
  | { kind: "hunk_anchor"; index: number }
  | { kind: "settings"; harness: AgentSlug; level: string }
  | { kind: "ask"; question: string; intent: string; parent_id: string | null }
  | { kind: "deep_dive" | "relabel_intent"; qa_id: string }
  | { kind: "keep_todo"; qa_id: string; title: string; notes: string };
export interface LearningHandoff {
  target: SessionTarget;
  draft_prompt: string;
  notice: string | null;
  info: string | null;
  /** The linked session still exists but is stopped; start it instead of opening another. */
  start_required: boolean;
}
export const learningBegin = (target: FeatureTarget): Promise<LearningView> =>
  invoke("learning_begin", { target });
export const learningSnapshot = (): Promise<LearningView | null> => invoke("learning_snapshot");
export const learningAct = (view: LearningView, action: LearningAction): Promise<LearningView | null> =>
  invoke("learning_act", { workflowId: view.workflow_id, revision: view.revision, action });
export const learningLaunchAgent = (view: LearningView, qaId: string, approved: boolean): Promise<LearningHandoff> =>
  invoke("learning_launch_agent", { workflowId: view.workflow_id, revision: view.revision, qaId, approved });

export interface DiffOptions {
  commit: string | null;
  base_ref: string | null;
  ignore_whitespace: boolean;
  context: "standard" | "expanded" | "full";
}
export interface DiffLine {
  kind: "context" | "added" | "removed" | "marker";
  text: string;
  old_line: number | null;
  new_line: number | null;
}
export interface DiffHunk { header: string; lines: DiffLine[] }
export interface DiffFile {
  path: string;
  old_path: string | null;
  status: string;
  additions: number;
  deletions: number;
  is_binary: boolean;
  hunks: DiffHunk[];
  patch: string;
}
export interface DiffView {
  target: FeatureTarget;
  feature_name: string;
  branch: string;
  base_ref: string;
  base_commit: string;
  commit: string | null;
  commits: { hash: string; short_hash: string; subject: string }[];
  commits_error: string | null;
  files: DiffFile[];
  total_additions: number;
  total_deletions: number;
}
export const loadDiff = (target: FeatureTarget, options: DiffOptions): Promise<DiffView> =>
  invoke("load_diff", { target, options });

export interface ReviewLocation { old_line: number | null; new_line: number | null }
export interface ReviewSpan { start: ReviewLocation; end: ReviewLocation }
export type ReviewSeverity = "blocker" | "suggestion" | "nit" | "question" | "praise";
export interface ReviewFile {
  diff: DiffFile;
  verdict: "approved" | "rejected" | "undecided";
  feedback: string;
  severity: ReviewSeverity;
  comment: { text: string; severity: ReviewSeverity; resolved: boolean; carried: boolean } | null;
  line_comments: (ReviewSpan & { editable: boolean; anchor: string; text: string; severity: ReviewSeverity; resolved: boolean; draft: boolean; anchor_lost: boolean; suggestion: string | null; apply_blocked: string | null })[];
  notes: string | null;
  walkthrough: string | null;
  changed_since_last: boolean;
}
export interface ReviewView {
  workflow_id: string;
  revision: number;
  target: FeatureTarget;
  feature_name: string;
  branch: string;
  base_ref: string;
  files: ReviewFile[];
  selected_path: string | null;
  general_feedback: string;
  has_prior_review: boolean;
  error: string | null;
  save_error: string | null;
  applied_suggestions: string[];
  history: ReviewHistory | null;
  summary: ReviewSummary | null;
  check_command: string | null;
  check: { command: string; status: "running" | "passed" | "failed" | "cancelled" | "stale"; output: string } | null;
  finish: ReviewFinish | null;
  ai: ReviewAi;
}
/** What completing would record and hand off; present with the summary. */
export interface ReviewFinish {
  approved: number;
  needs_work: number;
  skipped: number;
  file_comments: number;
  line_comments: number;
  general_feedback: boolean;
  /** Open suggestions a saved apply-on-finish opt-in writes before recording. */
  apply_suggestions: number;
  post_to_pr: boolean;
  /** Whether a handed-off prompt is submitted or left as an unsent draft. */
  submit_prompt: boolean;
  handoff: { session_id: string; label: string; stopped: boolean } | null;
  /** The configured check is running as part of a confirmed completion. */
  completing: boolean;
}
export interface ReviewCompletion {
  workflow_id: string;
  message: string;
  /** `draft_prompt` is unsent composer text; null when the prompt was submitted. */
  handoff: { target: SessionTarget; draft_prompt: string | null } | null;
}
export interface ReviewSummary {
  rows: { path: string | null; title: string; text: string; severity: ReviewSeverity | null; suggestion: string | null; apply_blocked: string | null }[];
  undecided: number;
  pending_suggestions: number;
  failures: string[];
}
export interface ReviewHistory {
  selected: number;
  rounds: { title: string; carried_unresolved: number }[];
  markdown: string;
  current_unresolved: number;
  archive_available: boolean;
  archive_loaded: boolean;
  error: string | null;
}
export interface ReviewAi {
  precall: PrecallView | null;
  running: boolean;
  walkthrough_path: string | null;
  co_review_path: string | null;
  overview_running: boolean;
  overview: string | null;
  question_running: boolean;
  questions: {
    question: string; answer: string | null; error: string | null; focus: string;
    path: string | null; start: ReviewLocation | null; end: ReviewLocation | null; harness: AgentSlug;
  }[];
  question_error: string | null;
  comment_draft: { request: number; turn: number; destination: "inline" | "general"; text: string } | null;
  ready_comment: { request: number; original: string; path: string | null; start: ReviewLocation | null; end: ReviewLocation | null; text: string; severity: ReviewSeverity } | null;
  harnesses: AgentSlug[];
  message: string | null;
}
export type ReviewAction =
  | { kind: "summary_open" | "summary_close" | "apply_finish_suggestions" | "cancel_check" }
  | { kind: "run_check"; command: string }
  | { kind: "complete"; check_command: string | null; apply_suggestions: number; handoff_session: string | null; deliver: boolean }
  | { kind: "history_open" | "history_load_older" | "history_close" }
  | { kind: "history_select"; round: number }
  | { kind: "select" | "approve" | "skip" | "toggle_resolved"; path: string }
  | { kind: "reject"; path: string; feedback: string; severity: ReviewSeverity }
  | { kind: "comment"; path: string; text: string; severity: ReviewSeverity }
  | (ReviewSpan & { kind: "line_comment"; path: string; text: string; severity: ReviewSeverity })
  | (ReviewSpan & { kind: "suggestion"; path: string; text: string })
  | (ReviewSpan & { kind: "toggle_line_resolved" | "apply_suggestion"; path: string })
  | { kind: "general"; text: string }
  | { kind: "walkthrough" | "co_review"; path: string }
  | { kind: "ask"; path: string; start: ReviewLocation | null; end: ReviewLocation | null; question: string; harness: AgentSlug }
  | { kind: "draft_question"; turn: number; destination: "inline" | "general" }
  | { kind: "transfer_question_draft"; request: number; text: string }
  | { kind: "discard_question_draft" }
  | (ReviewSpan & { kind: "accept_draft" | "dismiss_draft"; path: string })
  | { kind: "overview" | "cancel_ai" | "precall_confirm" | "precall_cancel" | "precall_toggle_view" }
  | { kind: "undo" | "refresh" | "reload" | "retry_save" | "pause" | "discard" };
export const reviewBegin = (target: FeatureTarget): Promise<ReviewView> => invoke("review_begin", { target });
/** Null once a confirmed completion finished while polling; take its result. */
export const reviewSnapshot = (workflowId: string): Promise<ReviewView | null> => invoke("review_snapshot", { workflowId });
/** A completed review's result, returned once. */
export const reviewTakeCompletion = (workflowId: string): Promise<ReviewCompletion | null> =>
  invoke("review_take_completion", { workflowId });
export const reviewAct = (view: ReviewView, action: ReviewAction): Promise<ReviewView | null> =>
  invoke("review_act", { workflowId: view.workflow_id, revision: view.revision, action });

export type LibraryScope = { kind: "global" } | { kind: "project"; project_id: string }
  | { kind: "feature"; project_id: string; feature_id: string };
export type LibrarySlot = {
  key: string; label: string; required: boolean; initial_value: string;
} & ({ kind: "text" | "multi_line"; default: string | null } | { kind: "select"; options: string[] });
export interface LibraryEntry {
  key: string; name: string; description: string | null; body: string; tags: string[];
  source: string; slots: LibrarySlot[];
}
export interface LibraryTarget { target: SessionTarget; label: string; stopped: boolean }
export interface LibraryView {
  entries: LibraryEntry[];
  /** Every key in the scope, ignoring the search: absence means changed or deleted. */
  available_keys: string[];
  targets: LibraryTarget[];
}
export interface ResolvePrompt {
  scope: LibraryScope; entry_key: string; values: [string, string][]; target: SessionTarget | null;
}
export function promptLibraryLoad(scope: LibraryScope, query: string): Promise<LibraryView> {
  return invoke("prompt_library_load", { scope, query });
}
export function promptLibraryResolve(request: ResolvePrompt): Promise<string> {
  return invoke("prompt_library_resolve", { request });
}

// Mirrors src/gui_dormancy.rs.
/** The readings a dormant row was listed on; sent back verbatim to stop it. */
export interface DormantObservation {
  target: FeatureTarget;
  feature_name: string;
  tmux_session: string;
  last_activity: string;
  last_accessed: string;
}
export interface DormantFeatureView {
  project_name: string;
  workdir: string;
  is_worktree: boolean;
  editor_alive: boolean;
  idle_secs: number;
  unattended_secs: number;
  observation: DormantObservation;
}
export interface DormancyView {
  /** False when either threshold is 0 in config, which switches detection off. */
  enabled: boolean;
  idle_minutes: number;
  unattended_hours: number;
  kill_editor_on_stop: boolean;
  checked_at: string;
  features: DormantFeatureView[];
}
export interface EditorCleanup {
  killed: { name: string; processes: number }[];
  /** `deliberate` is false only for an editor that had already closed. */
  skipped: { name: string; reason: string; deliberate: boolean }[];
  /** Windows still opening; AMF closes them once they can be identified. */
  pending: string[];
  summary: string | null;
}
export type DormancyRefusal = "duplicate" | "deleted" | "already_stopped" | "session_gone"
  | "restarted" | "shared_session" | "dormancy_off" | "opened" | "output" | "no_longer_dormant";
export type DormancyStopResult = { target: FeatureTarget; feature_name: string } & (
  | { outcome: "stopped"; editors: EditorCleanup | null }
  | { outcome: "refused"; reason: DormancyRefusal; message: string }
  | { outcome: "failed"; message: string });
export const dormancyLoad = (): Promise<DormancyView> => invoke("dormancy_load");
export const dormancyStop = (selection: DormantObservation[]): Promise<DormancyStopResult[]> =>
  invoke("dormancy_stop", { selection });
