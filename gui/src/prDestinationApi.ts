import { invoke } from "@tauri-apps/api/core";
import type { AgentSlug, ModeSlug, SessionTarget } from "./api";
import type { FixDestinationKind } from "./prTriageApi";

/** Which PR workflow a destination step belongs to. */
export type PrDestinationWorkflow = { kind: "triage" | "workspace"; workflow_id: string };
/** What the person was looking at; echoed back so a changed PR is a conflict. */
export interface PrIdentity { pr_number: number; head_sha: string; set_digest: string | null }
export interface CompanionSettings { branch: string; agent: AgentSlug; mode: ModeSlug; review: boolean; enable_chrome: boolean }
export type PrDestinationChoice =
  | { kind: "dedicated_session"; agent: AgentSlug; name: string | null }
  | { kind: "companion_feature"; settings: CompanionSettings; hook_choice: string | null };

export interface DedicatedAgent { agent: AgentSlug; label: string; unavailable: string | null }
export interface DedicatedCatalog {
  identity: PrIdentity; feature_name: string; default_name: string; agents: DedicatedAgent[]; preferred: AgentSlug;
}
/** The project's on_worktree_created hook exactly as it would run. */
export interface HookPreview { script: string; prompt_title: string | null; options: string[]; choice: string | null; choice_required: boolean }
export interface CompanionPreview {
  branch: string; destination_path: string; source_path: string; head_sha: string; base_sha: string; base_short: string;
  pr_branch: string; pr_branch_on_origin: boolean | null; pr_branch_note: string | null; will_fetch: boolean;
  agent: AgentSlug; mode: ModeSlug; review: boolean; enable_chrome: boolean; hook: HookPreview | null; older_companions: string[];
}
export interface ModeChoice { mode: ModeSlug; label: string; description: string; unsupported_agents: AgentSlug[] }
export interface CompanionCatalog {
  identity: PrIdentity; source_name: string; source_branch: string; project_name: string; pr_branch: string; suggested_branch: string;
  agents: DedicatedAgent[]; preferred: AgentSlug; default_mode: ModeSlug; modes: ModeChoice[]; hook: HookPreview | null;
}
export interface PrDestinationOperation {
  operation_id: string; state: "running" | "created" | "failed" | "cancelled";
  phase: "starting" | "fetching" | "creating_worktree" | "running_hook" | "registering" | "done";
  cancel_requested: boolean; message: string; retained_path: string | null; created_target: SessionTarget | null;
  destination_revision: string | null; destination_kind: FixDestinationKind; stopped: boolean; hook_warning: string | null;
}
export interface PrDestinationPreview {
  companion?: CompanionPreview;
  revision: string; identity: PrIdentity; label: string; agent: AgentSlug; feature_name: string; workdir: string;
  reuses_existing: boolean; existing_target: SessionTarget | null; creates_session: boolean; stopped: boolean;
  resource_warning: string | null; consequences: string[];
}
export interface PrDestinationPrepared {
  status: "created" | "reused" | "in_progress" | "started"; operation_id: string; created_target: SessionTarget | null;
  destination_revision: string | null; destination_kind: FixDestinationKind; stopped: boolean; message: string;
}

export const dedicatedCatalog = (workflow: PrDestinationWorkflow) =>
  invoke<DedicatedCatalog>("pr_dedicated_catalog", { workflow });
export const destinationPreview = (workflow: PrDestinationWorkflow, identity: PrIdentity, choice: PrDestinationChoice) =>
  invoke<PrDestinationPreview>("pr_destination_preview", { request: { workflow, identity, choice } });
export const destinationPrepare = (
  workflow: PrDestinationWorkflow, identity: PrIdentity, choice: PrDestinationChoice, previewRevision: string, approved: boolean,
) => invoke<PrDestinationPrepared>("pr_destination_prepare", {
  request: { workflow, identity, choice, preview_revision: previewRevision, approved },
});

export const companionCatalog = (workflow: PrDestinationWorkflow) =>
  invoke<CompanionCatalog>("pr_companion_catalog", { workflow });
export const destinationStatus = (operationId: string) =>
  invoke<PrDestinationOperation>("pr_destination_status", { operationId });
export const destinationCancel = (operationId: string) =>
  invoke<PrDestinationOperation>("pr_destination_cancel", { operationId });
