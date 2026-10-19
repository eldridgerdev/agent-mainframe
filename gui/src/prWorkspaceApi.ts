import { invoke } from "@tauri-apps/api/core";
import type { DiffFile, FeatureTarget } from "./api";
import type { PrTriageView } from "./prTriageApi";

export interface WorkspaceFiles { workflow_id: string; pr_number: number; head_sha: string; files: DiffFile[] }
export interface WorkspaceFinding { path: string | null; line: number | null; side: string | null; body: string; diff_hunk: string | null; skipped: boolean; published: boolean }
export interface WorkspaceAi {
  workflow_id: string; pr_number: number; head_sha: string; running: boolean; stage: string | null;
  activity: string | null; token_estimate: number | null; findings: WorkspaceFinding[]; summary: string | null;
  error: string | null; precall: { title: string; harness: string; preview: string; viewing: boolean } | null; can_cancel: boolean;
}
export type WorkspaceAiAction = { kind: "start" | "cancel" | "confirm_precall" | "cancel_precall" };
export const workspaceBegin = (target: FeatureTarget) => invoke<string>("pr_workspace_begin", { target });
export const workspaceSelectPr = (workflowId: string, number: number) => invoke<void>("pr_workspace_select_pr", { workflowId, number });
export const workspaceSwitchAi = (workflowId: string, number: number) => invoke<void>("pr_workspace_switch_ai", { workflowId, number });
export const workspaceSwitchThreads = (workflowId: string) => invoke<void>("pr_workspace_switch_threads", { workflowId });
export const workspaceClose = (workflowId: string) => invoke<void>("pr_workspace_close", { workflowId });
export const workspaceFiles = (workflowId: string) => invoke<WorkspaceFiles>("pr_workspace_files", { workflowId });
export const workspaceAiSnapshot = (workflowId: string) => invoke<WorkspaceAi>("pr_workspace_ai_snapshot", { workflowId });
export const workspaceAiAct = (workflowId: string, action: WorkspaceAiAction) => invoke<WorkspaceAi>("pr_workspace_ai_act", { workflowId, action });
export type { PrTriageView };
