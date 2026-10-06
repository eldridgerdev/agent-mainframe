import { invoke } from "@tauri-apps/api/core";
import type { DiffFile, DiffOptions, FeatureTarget } from "./api";

// Mirrors `src/gui_supervised_edits.rs`. Kept beside `api.ts` rather than in
// it so this workflow's contract stays in one reviewable place.

export const MAX_FEEDBACK_CHARS = 200;

export interface DecisionEffects {
  approve: string;
  reject: string;
  cancel: string;
  /** OpenCode blocks a rejected write but does not forward the feedback. */
  feedback_reaches_agent: boolean;
}

export interface SupervisedEdit {
  id: string;
  revision: string;
  kind: "diff-review" | "change-reason" | string;
  path: string;
  tool: string;
  is_new_file: boolean;
  agent_reason: string | null;
  diff: DiffFile | null;
  diff_error: string | null;
  old_snippet: string | null;
  new_snippet: string | null;
  requested_at: number | null;
  /** An answer was delivered and the agent has not picked it up yet. */
  answered: boolean;
  unavailable: string | null;
  effects: DecisionEffects;
}

export interface SupervisedEditsView {
  target: FeatureTarget;
  feature_name: string;
  edits: SupervisedEdit[];
}

export interface PendingEditCount {
  project_id: string;
  feature_id: string;
  feature_name: string;
  count: number;
  first_id: string;
  first_path: string;
}

export type SupervisedEditDecision =
  | { kind: "approve" }
  | { kind: "reject"; feedback: string }
  | { kind: "cancel" };

export interface SupervisedEditOutcome {
  message: string;
  view: SupervisedEditsView;
}

export const supervisedEditsLoad = (
  target: FeatureTarget,
  context: DiffOptions["context"],
): Promise<SupervisedEditsView> => invoke("supervised_edits_load", { target, context });

export const supervisedEditCounts = (): Promise<PendingEditCount[]> =>
  invoke("supervised_edit_counts");

export const supervisedEditRespond = (
  target: FeatureTarget,
  edit: Pick<SupervisedEdit, "id" | "revision">,
  decision: SupervisedEditDecision,
): Promise<SupervisedEditOutcome> =>
  invoke("supervised_edit_respond", { target, editId: edit.id, revision: edit.revision, decision });
