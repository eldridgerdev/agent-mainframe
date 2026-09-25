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
}

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
}

export interface Project {
  id: string;
  name: string;
  repo: string;
  is_git: boolean;
  features: Feature[];
}

export interface WorkspaceSnapshot {
  projects: Project[];
  snapshot_at: string;
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
