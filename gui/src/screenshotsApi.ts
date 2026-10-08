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
export interface RemoteItem { key: string; caption: string; provenance: string[] }
export interface RunChoice { id: number; attempt: number; name: string; head_sha: string; status: string; conclusion: string; created_at: string }
export interface RemoteListing { request_id: string; items: RemoteItem[]; galleries: { url: string; reason: string; provenance: string }[]; issues: { source: string; message: string }[]; runs: RunChoice[]; selected_run: number | null; run_page: number; more_runs: boolean }
export const remoteScreenshots = (workflowId: string, selectedRun: number | null, runPage: number, requestId: string) => invoke<RemoteListing>("screenshots_remote_list", { workflowId, selectedRun, runPage, requestId });
export const remoteScreenshotImage = (requestId: string, key: string, thumbnail: boolean) => invoke<ImageData>("screenshots_remote_image", { requestId, key, thumbnail });
export const closeRemoteScreenshots = (requestId: string) => invoke<void>("screenshots_remote_close", { requestId });
export const openScreenshotBrowser = (url: string) => invoke<void>("screenshots_open_browser", { url });
