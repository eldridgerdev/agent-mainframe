// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import PrWorkspacePanel from "../src/PrWorkspacePanel";
import type { PrTriageView } from "../src/prTriageApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); vi.useRealTimers(); });

const target = { project_id: "p", feature_id: "f" };
const picker: PrTriageView = {
  workflow_id: "threads", revision: 1, target, feature_name: "Feature", branch: "feature",
  stage: "pick", picker: { entries: [{ number: 42, title: "Add feature", author: "dev", head_ref: "feature", updated_at: "", is_draft: false, state: "OPEN", mine: true, selected_for_feature: true }], include_closed: false, error: null, branch_pr: 42, loading: false },
  loading_pr: null, review: null, precall: null, reply: null, write_confirm: null, fix_targets: [], fix_draft: null, handoff: null,
  harnesses: ["claude"], default_harness: "claude", error: null, notice: null,
};
const reviewing: PrTriageView = {
  ...picker, revision: 2, stage: "review", picker: null,
  review: { number: 42, url: "", head_sha: "abcdef012345", head_ref: "feature", branch_mismatch: null, fetched_at: "now", open_count: 1, total: 1, hide_resolved: false, sort: "fetch_order", hidden_resolved: 0, conversation_start: null, investigating: null, investigating_harness: null,
    comments: [{ id: 7, kind: "inline", review_state: null, author: "reviewer", is_bot: false, path: "src/lib.rs", line: 4, side: "RIGHT", outdated: false, file_level: false, body: "Check this change", snippet: "Check this change", hunk: null, resolved: false, can_resolve: true, triage: "untriaged", local_note: null, actionable: true, local_finding: false, replies: [], investigation: null }] },
};

it("opens the feature PR in a shared Files/Threads/AI workspace and cancels a running review", async () => {
  let running = false;
  let picked = false;
  vi.mocked(invoke).mockImplementation(async (command: string, args?: unknown) => {
    if (command === "pr_workspace_begin") return "workspace";
    if (command === "pr_triage_begin") return picker;
    if (command === "pr_triage_snapshot") return picked ? reviewing : picker;
    if (command === "pr_triage_act") { picked = true; return reviewing; }
    if (command === "pr_workspace_select_pr" || command === "pr_workspace_switch_ai" || command === "pr_workspace_switch_threads" || command === "pr_workspace_close") return undefined;
    if (command === "pr_workspace_files") return { workflow_id: "workspace", pr_number: 42, head_sha: "abcdef012345", files: [{ path: "src/lib.rs", old_path: null, status: "modified", additions: 1, deletions: 0, is_binary: false, hunks: [], patch: "", syntax: null }] };
    if (command === "pr_workspace_ai_snapshot" || command === "pr_workspace_ai_act") {
      const kind = (args as { action?: { kind: string } } | undefined)?.action?.kind;
      if (kind === "start") running = true;
      if (kind === "cancel") running = false;
      return { workflow_id: "workspace", pr_number: 42, head_sha: "abcdef012345", running, stage: running ? "Reviewing code" : null, activity: "Reviewing src/lib.rs (initial)", elapsed_seconds: 15, token_estimate: 900, findings: [], summary: null, error: null, precall: null, can_cancel: running };
    }
    throw new Error(`Unexpected command ${command}`);
  });

  render(<PrWorkspacePanel target={target} onClose={vi.fn()} />);
  const collapseSidebar = await screen.findByRole("button", { name: "Collapse PR sidebar" });
  expect(collapseSidebar.getAttribute("aria-expanded")).toBe("true");
  fireEvent.click(collapseSidebar);
  expect(screen.getByRole("button", { name: "Expand PR sidebar" }).getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(screen.getByRole("button", { name: "Expand PR sidebar" }));
  fireEvent.click(await screen.findByRole("button", { name: /#42 Add feature/ }));
  expect(await screen.findByLabelText("Sort")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Investigate…" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Reply: fixed" })).toBeTruthy();
  fireEvent.click(await screen.findByRole("button", { name: "Files" }));
  expect((await screen.findAllByText("src/lib.rs")).length).toBeGreaterThan(0);
  fireEvent.click(screen.getByRole("button", { name: "AI Review" }));
  fireEvent.click(await screen.findByRole("button", { name: "Start AI review" }));
  expect(await screen.findByText("Analyze the code")).toBeTruthy();
  expect(screen.getByText("Fetch PR changes")).toBeTruthy();
  expect(screen.getByText("Reviewing src/lib.rs (initial)")).toBeTruthy();
  expect(screen.getByText("15s")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Cancel AI review" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Start AI review" })).toBeTruthy());
  expect(vi.mocked(invoke).mock.calls.some(([name, args]) => name === "pr_workspace_ai_act" && (args as { action: { kind: string } }).action.kind === "cancel")).toBe(true);
});

it("does not overlap slow PR thread snapshots", async () => {
  const loading: PrTriageView = { ...picker, revision: 2, stage: "loading", picker: null, loading_pr: 42 };
  let resolveSnapshot!: (value: PrTriageView) => void;
  let snapshots = 0;
  vi.mocked(invoke).mockImplementation(async (command: string) => {
    if (command === "pr_workspace_begin") return "workspace";
    if (command === "pr_triage_begin") return picker;
    if (command === "pr_triage_act") return loading;
    if (command === "pr_triage_snapshot") {
      snapshots += 1;
      return new Promise<PrTriageView>((resolve) => { resolveSnapshot = resolve; });
    }
    if (command === "pr_workspace_select_pr" || command === "pr_workspace_close") return undefined;
    throw new Error(`Unexpected command ${command}`);
  });

  render(<PrWorkspacePanel target={target} onClose={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: /#42 Add feature/ }));
  await waitFor(() => expect(screen.getByText(/Loading review comments for PR #42/)).toBeTruthy());
  await waitFor(() => expect(snapshots).toBe(1), { timeout: 1500 });
  await new Promise((resolve) => window.setTimeout(resolve, 800));
  expect(snapshots).toBe(1);

  await act(async () => resolveSnapshot(reviewing));
});
