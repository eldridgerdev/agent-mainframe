import { invoke } from "@tauri-apps/api/core";
import type { AgentSlug, FeatureTarget } from "./api";

export interface HarnessChangeView {
  target: FeatureTarget;
  feature_name: string;
  current_agent: AgentSlug;
  running: boolean;
  agent_rows: number;
  choices: { agent: AgentSlug; name: string; unavailable: string | null }[];
  revision: string;
}
export const featureHarnessLoad = (target: FeatureTarget) =>
  invoke<HarnessChangeView>("feature_harness_load", { target });
export const featureHarnessApply = (target: FeatureTarget, revision: string, agent: AgentSlug) =>
  invoke<void>("feature_harness_apply", { request: { target, revision, agent } });
