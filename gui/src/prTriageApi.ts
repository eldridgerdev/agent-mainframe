import { invoke } from "@tauri-apps/api/core";
import type { AgentSlug, PrecallView, SessionTarget } from "./api";

// Mirrors `src/gui_pr_triage.rs`. Kept beside, not inside, api.ts so the PR
// Triage contract can grow without touching the shared type module.

export interface PrTriageTarget { project_id: string; feature_id?: string | null }

export interface PrEntry {
  number: number;
  title: string;
  author: string;
  head_ref: string;
  updated_at: string;
  is_draft: boolean;
  state: string;
  mine: boolean;
}

export interface PrPicker {
  entries: PrEntry[];
  include_closed: boolean;
  error: string | null;
  branch_pr: number | null;
  /** The list is still being read; keep polling. */
  loading: boolean;
}

export interface PrThreadReply { id: number; author: string; body: string; via_amf: boolean }

export interface PrInvestigation {
  status: "running" | "complete" | "failed" | "dismissed";
  harness: AgentSlug;
  answer: string | null;
  error: string | null;
  follow_ups: { question: string; answer: string; harness: AgentSlug }[];
  stale_head: boolean;
}

export interface PrComment {
  id: number;
  kind: "inline" | "review_summary" | "conversation";
  review_state: string | null;
  author: string;
  is_bot: boolean;
  path: string | null;
  line: number | null;
  side: string | null;
  outdated: boolean;
  file_level: boolean;
  body: string;
  snippet: string;
  hunk: string | null;
  resolved: boolean;
  can_resolve: boolean;
  triage: "untriaged" | "fixing" | "done" | "skipped" | "replied";
  local_note: string | null;
  actionable: boolean;
  local_finding: boolean;
  replies: PrThreadReply[];
  investigation: PrInvestigation | null;
}

export type PrSort = "fetch_order" | "by_file" | "by_author" | "humans_first" | "conversations";

export interface PrReview {
  number: number;
  url: string;
  head_sha: string;
  head_ref: string;
  branch_mismatch: string | null;
  fetched_at: string;
  open_count: number;
  total: number;
  hide_resolved: boolean;
  sort: PrSort;
  hidden_resolved: number;
  conversation_start: number | null;
  comments: PrComment[];
  investigating: number | null;
  investigating_harness: AgentSlug | null;
}

export type PrReplyKind = "done" | "not_needed" | "investigation";

export interface PrFixTarget { target: SessionTarget; label: string; harness: AgentSlug; stopped: boolean }
export interface PrFixHandoff { target: SessionTarget; draft_prompt: string }

export interface PrTriageView {
  workflow_id: string;
  revision: number;
  target: PrTriageTarget;
  feature_name: string;
  branch: string;
  stage: "pick" | "loading" | "review";
  picker: PrPicker | null;
  loading_pr: number | null;
  review: PrReview | null;
  precall: PrecallView | null;
  reply: { comment_id: number; kind: PrReplyKind; seed: string; agent_drafted: boolean } | null;
  write_confirm: { kind: "reply" | "resolve" | "reopen"; comment_id: number; destination: string; body: string | null } | null;
  fix_targets: PrFixTarget[];
  fix_draft: { comment_id: number; target: SessionTarget; prompt: string; submission_prompt?: string | null } | null;
  handoff: PrFixHandoff | null;
  harnesses: AgentSlug[];
  default_harness: AgentSlug | null;
  error: string | null;
  notice: string | null;
}

export type PrTriageAction =
  | { kind: "toggle_closed" | "back_to_list" | "refresh" | "precall_toggle_view" | "precall_cancel" | "precall_confirm"
      | "confirm_fix_submission" | "cancel_fix_submission" | "cancel_fix_draft" | "cancel_investigation" | "discard_reply" | "cancel_write" | "confirm_write" | "close" }
  | { kind: "start_fix_draft"; comment_id: number; session_id: string }
  | { kind: "confirm_fix_draft" | "prepare_fix_submission"; prompt: string }
  | { kind: "open"; number: number }
  | { kind: "view"; hide_resolved: boolean; sort: PrSort }
  | { kind: "toggle_done" | "toggle_skipped" | "dismiss_investigation" | "request_resolve"; comment_id: number }
  | { kind: "investigate"; comment_id: number; harness: AgentSlug; note: string | null; follow_up: string | null }
  | { kind: "start_reply"; comment_id: number; reply: PrReplyKind }
  | { kind: "prepare_reply"; comment_id: number; body: string };

export function prTriageBegin(target: PrTriageTarget): Promise<PrTriageView> {
  return invoke("pr_triage_begin", { target });
}

export function prTriageSnapshot(workflowId: string): Promise<PrTriageView> {
  return invoke("pr_triage_snapshot", { workflowId });
}

export function prTriageAct(view: PrTriageView, action: PrTriageAction): Promise<PrTriageView | null> {
  return invoke("pr_triage_act", { workflowId: view.workflow_id, revision: view.revision, action });
}
