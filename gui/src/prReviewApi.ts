import { invoke } from "@tauri-apps/api/core";
import type { DiffFile } from "./api";

export interface PrReviewView {
  workflow_id: string;
  revision: number;
  project_name: string;
  stage: "pick" | "review";
  entries: { number: number; title: string; author: string; head_ref: string; is_draft: boolean; has_draft: boolean; updated: boolean }[];
  loading: boolean;
  number: number | null;
  title: string | null;
  files: { diff: DiffFile; comment: string }[];
  summary: string;
  submission: {
    event: string; body: string;
    comments: { path: string; line: number; side: string; start_line: number | null; body: string }[];
    file_comments: { path: string; body: string }[];
    posting: boolean; error: string | null; head_moved: boolean;
  } | null;
  error: string | null;
  notice: string | null;
}
export type PrReviewAction =
  | { kind: "retry" | "back" | "close" | "cancel_submit" | "confirm_submit" | "reopen" }
  | { kind: "open"; number: number }
  | { kind: "file_comment"; path: string; text: string }
  | { kind: "summary"; text: string }
  | { kind: "preview"; event: string };
export const prReviewBegin = (projectId: string): Promise<PrReviewView> => invoke("pr_review_begin", { projectId });
export const prReviewSnapshot = (workflowId: string): Promise<PrReviewView> => invoke("pr_review_snapshot", { workflowId });
export const prReviewAct = (view: PrReviewView, action: PrReviewAction): Promise<PrReviewView | null> => invoke("pr_review_act", { workflowId: view.workflow_id, revision: view.revision, action });
