import { invoke } from "@tauri-apps/api/core";
import type { DiffFile, FeatureTarget, SessionTarget } from "./api";
import type { PrReviewView } from "./prReviewApi";
import type { PrTriageView, PrFixHandoff, PrFixTarget } from "./prTriageApi";

export interface WorkspaceFiles { workflow_id: string; pr_number: number; head_sha: string; files: DiffFile[] }
export interface WorkspaceFinding { path: string | null; line: number | null; side: string | null; body: string; diff_hunk: string | null; skipped: boolean; published: boolean }
export interface WorkspaceAi {
  workflow_id: string; pr_number: number; head_sha: string; running: boolean; stage: string | null;
  activity: string | null; elapsed_seconds: number; token_estimate: number | null; findings: WorkspaceFinding[]; summary: string | null;
  result_revision: string | null; error: string | null; precall: { title: string; harness: string; preview: string; viewing: boolean } | null; can_cancel: boolean;
}
export type WorkspaceAiAction = { kind: "start" | "cancel" | "confirm_precall" | "cancel_precall" | "post" };
export const workspaceBegin = (target: FeatureTarget) => invoke<string>("pr_workspace_begin", { target });
export const workspaceSelectPr = (workflowId: string, number: number) => invoke<void>("pr_workspace_select_pr", { workflowId, number });
export const workspaceSwitchAi = (workflowId: string, number: number) => invoke<void>("pr_workspace_switch_ai", { workflowId, number });
export const workspaceSwitchThreads = (workflowId: string) => invoke<void>("pr_workspace_switch_threads", { workflowId });
export const workspaceClose = (workflowId: string) => invoke<void>("pr_workspace_close", { workflowId });
export const workspaceReviewOpen = (workflowId: string, number: number) => invoke<PrReviewView>("pr_workspace_review_open", { workflowId, number });
export const workspaceReviewClose = (workflowId: string) => invoke<void>("pr_workspace_review_close", { workflowId });
export const workspaceFiles = (workflowId: string) => invoke<WorkspaceFiles>("pr_workspace_files", { workflowId });
export const workspaceAiSnapshot = (workflowId: string) => invoke<WorkspaceAi>("pr_workspace_ai_snapshot", { workflowId });
export const workspaceAiAct = (workflowId: string, action: WorkspaceAiAction) => invoke<WorkspaceAi>("pr_workspace_ai_act", { workflowId, action });
export type { PrTriageView };

export const workspacePrepareFix = (request: { workflow_id: string; result_revision: string; finding_indices: number[]; target: SessionTarget; destination_revision: string }) => invoke<PrFixHandoff>("pr_workspace_prepare_fix", { request });
export const workspaceFixTargets = (workflowId: string) => invoke<PrFixTarget[]>("pr_workspace_fix_targets", { workflowId });
