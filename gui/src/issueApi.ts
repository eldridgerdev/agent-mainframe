import { invoke } from "@tauri-apps/api/core";
import type { CreateFeatureRequest, SessionTarget } from "./api";

// Mirrors `src/gui_issues.rs`. Every call here is a read-only GitHub request
// except `issueFixCreate`, which makes a local feature and writes nothing to
// GitHub.

export interface LinkedFeature { project_name: string; feature_name: string }

export interface IssueRow {
  number: number;
  title: string;
  labels: string[];
  updated_at: string;
  url: string;
  excerpt: string;
  linked_features: LinkedFeature[];
}

export interface IssueListView {
  project_id: string;
  project_name: string;
  repository: string;
  page: number;
  per_page: number;
  has_next_page: boolean;
  issues: IssueRow[];
}

export interface IssuePreview {
  project_id: string;
  project_name: string;
  repository: string;
  number: number;
  title: string;
  labels: string[];
  updated_at: string;
  url: string;
  body: string;
  body_truncated: boolean;
  /** Binds the confirmation to exactly this issue text. Sent back unchanged. */
  digest: string;
  prompt: string;
  suggested_feature_name: string;
  suggested_branch: string;
  linked_features: LinkedFeature[];
}

export interface IssueFixRequest {
  project_id: string;
  number: number;
  digest: string;
  feature_name: string;
  request: CreateFeatureRequest;
  prompt: string;
  start: boolean;
  approved: boolean;
  allow_duplicate: boolean;
}

export interface IssueFixResponse {
  project_id: string;
  feature_id: string;
  feature_name: string;
  branch: string;
  started: boolean;
  session: SessionTarget | null;
  draft_prompt: string;
  message: string;
}

export function issueBrowserList(projectId: string, page: number): Promise<IssueListView> {
  return invoke("issue_browser_list", { projectId, page });
}

export function issueFixPreview(projectId: string, number: number): Promise<IssuePreview> {
  return invoke("issue_fix_preview", { projectId, number });
}

export function issueFixCreate(request: IssueFixRequest): Promise<IssueFixResponse> {
  return invoke("issue_fix_create", { request });
}
