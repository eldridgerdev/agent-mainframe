import { invoke } from "@tauri-apps/api/core";

export interface SteeringCheck {
  key: string;
  label: string;
  present: boolean;
  /** "covered", or what the draft is missing (the TUI's wording). */
  detail: string;
}

/** The local steering-coach checklist for one exact draft text. */
export interface SteeringAnalysis {
  /** SHA-256 of the exact analyzed text; identifies what this describes. */
  digest: string;
  score: number;
  max_score: number;
  summary: string;
  checks: SteeringCheck[];
  tips: string[];
  blank: boolean;
}

/** Pure local scan: no harness call, nothing is sent anywhere. */
export function steeringAnalyze(text: string): Promise<SteeringAnalysis> {
  return invoke("steering_analyze", { text });
}
