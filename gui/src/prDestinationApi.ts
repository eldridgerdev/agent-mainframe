import { invoke } from "@tauri-apps/api/core";
import type { AgentSlug, SessionTarget } from "./api";
import type { FixDestinationKind } from "./prTriageApi";

/** Which PR workflow a destination step belongs to. */
export type PrDestinationWorkflow = { kind: "triage" | "workspace"; workflow_id: string };
/** What the person was looking at; echoed back so a changed PR is a conflict. */
export interface PrIdentity { pr_number: number; head_sha: string; set_digest: string | null }
export type PrDestinationChoice = { kind: "dedicated_session"; agent: AgentSlug; name: string | null };

export interface DedicatedAgent { agent: AgentSlug; label: string; unavailable: string | null }
export interface DedicatedCatalog {
  identity: PrIdentity; feature_name: string; default_name: string; agents: DedicatedAgent[]; preferred: AgentSlug;
}
export interface PrDestinationPreview {
  revision: string; identity: PrIdentity; label: string; agent: AgentSlug; feature_name: string; workdir: string;
  reuses_existing: boolean; existing_target: SessionTarget | null; creates_session: boolean; stopped: boolean;
  resource_warning: string | null; consequences: string[];
}
export interface PrDestinationPrepared {
  status: "created" | "reused" | "in_progress"; operation_id: string; created_target: SessionTarget | null;
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
