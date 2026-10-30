import { invoke } from "@tauri-apps/api/core";
import type { FeatureTarget } from "./api";

export type ExplanationView =
  | { state: "saved_note"; text: string }
  | { state: "precall"; owner: string; prompt: string; model: string | null; reasoning: string | null }
  | { state: "running"; owner: string; request: number }
  | { state: "complete"; owner: string; request: number; text: string }
  | { state: "failed"; owner: string; request: number; message: string }
  | { state: "cancelled"; owner: string };
export const explanationPrepare = (target: FeatureTarget, editId: string, revision: string) =>
  invoke<ExplanationView>("supervised_edit_explanation_prepare", { target, editId, revision });
export const explanationStart = (owner: string, approved = false) =>
  invoke<ExplanationView>("supervised_edit_explanation_start", { owner, approved });
export const explanationPoll = (owner: string, request: number) =>
  invoke<ExplanationView>("supervised_edit_explanation_poll", { owner, request });
export const explanationCancel = (owner: string) =>
  invoke<ExplanationView>("supervised_edit_explanation_cancel", { owner });
