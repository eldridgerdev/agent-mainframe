import { invoke } from "@tauri-apps/api/core";
import type { AgentSlug, FeatureTarget, ModeSlug } from "./api";

// Mirrors src/gui_fork.rs. Fork is a two-step operation: `forkPrepare` reads
// the source and returns a token-bound preview; `forkConfirm` must echo the
// identities the user saw. A token is single use.

export type ForkChangeKind = "file" | "executable" | "symlink" | "deleted";
export type ContextStatus = "available" | "no_session" | "unsupported_harness" | "not_bound" | "unreadable";
export type ForkPhase = "checking" | "creating_checkout" | "transferring" | "registering" | "hook" | "starting";

export interface ForkPreview {
  token: string;
  source: {
    project_id: string;
    project_name: string;
    feature_id: string;
    feature_name: string;
    branch: string;
    workdir: string;
    head: string;
    session: { id: string; label: string; kind: string } | null;
  };
  suggested_branch: string;
  default_agent: AgentSlug;
  mode: ModeSlug;
  changes: { total: number; listed: { path: string; kind: ForkChangeKind }[]; truncated: boolean };
  staging_note: string;
  context: { status: ContextStatus; message: string; exported_chars: number | null };
  hook: { title: string; options: string[] } | null;
}

export interface ForkConfirmRequest {
  token: string;
  expected: { project_id: string; feature_id: string; session_id: string | null; branch: string; head: string };
  new_branch: string;
  agent: AgentSlug;
  include_context: boolean;
  hook_choice: string | null;
  start_agent: boolean;
  approve_resources: boolean;
}

export interface ForkHookReport { succeeded: boolean; cancelled: boolean; detail: string | null }
export interface ForkRetained { workdir: string; branch: string }

export type ForkOutcome =
  | {
      status: "created";
      feature: FeatureTarget;
      branch: string;
      workdir: string;
      started: boolean;
      transferred: number;
      context_written: boolean;
      hook: ForkHookReport | null;
      notes: string[];
      message: string;
    }
  | { status: "cancelled"; message: string }
  | { status: "failed"; phase: ForkPhase; retained: ForkRetained | null; feature: FeatureTarget | null; message: string };

export interface ForkStatus {
  state: "prepared" | "running" | "done";
  phase: ForkPhase | null;
  cancel_requested: boolean;
  outcome: ForkOutcome | null;
}

export const forkPrepare = (target: FeatureTarget, sessionId: string | null): Promise<ForkPreview> =>
  invoke("fork_prepare", { request: { target, session_id: sessionId } });
export const forkConfirm = (request: ForkConfirmRequest): Promise<ForkOutcome> =>
  invoke("fork_confirm", { request });
export const forkCancel = (token: string): Promise<ForkStatus> => invoke("fork_cancel", { token });
export const forkStatus = (token: string): Promise<ForkStatus> => invoke("fork_status", { token });
