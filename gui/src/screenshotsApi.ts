import { invoke } from "@tauri-apps/api/core";
import type { FeatureTarget } from "./api";
export interface EvidenceOwner {
  version: number; scope_id: string; project_id: string; feature_id: string; session_id: string;
  project_name: string; feature_name: string; session_label: string; workdir: string; is_worktree: boolean; created_at: string;
}
export interface EvidenceItem {
  key: string; scope_id: string; image_id: string; file: string; sha256: string; caption: string; captured_at: string; owner: EvidenceOwner;
}
export interface EvidenceListing {
  items: EvidenceItem[]; issues: { scope_id: string; file: string; message: string }[]; owners: EvidenceOwner[]; truncated: boolean;
}
export interface ImageData { data_url: string; width: number; height: number }
export interface EvidenceSelection { feature: FeatureTarget | null; session_id: string | null }
export const screenshotsList = (selection: EvidenceSelection) => invoke<EvidenceListing>("screenshots_list", { selection });
export const screenshotsChanged = () => invoke<boolean>("screenshots_changed");
export const screenshotImage = (item: EvidenceItem, thumbnail: boolean) => invoke<ImageData>("screenshots_image", { scopeId: item.scope_id, imageId: item.image_id, hash: item.sha256, thumbnail });
export const screenshotCleanupScopes = () => invoke<[EvidenceOwner, boolean][]>("screenshots_cleanup_scopes");
export const screenshotCleanup = (scopeId: string) => invoke<void>("screenshots_cleanup", { scopeId });
export const openScreenshotBrowser = (url: string) => invoke<void>("screenshots_open_browser", { url });
export const prDescription = (workflowId: string) => invoke<string>("screenshots_pr_document", { workflowId });
export const inlinePrImage = (workflowId: string, source: string) => invoke<ImageData>("screenshots_inline_image", { workflowId, source });
export interface GithubAccessCheck { name: string; passed: boolean; detail: string }
export const checkPrImageAccess = (workflowId: string, source: string) => invoke<GithubAccessCheck[]>("screenshots_check_access", { workflowId, source });
