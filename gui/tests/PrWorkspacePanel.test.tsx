// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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

it("posts findings only after confirmation and prepares a selected or batch fix in the agent composer", async () => {
  const fixes: PrTriageView = { ...reviewing, fix_targets: [{ target: { ...target, session_id: "session-1" }, label: "Claude 1", harness: "claude", stopped: false, destination_revision: "target-rev" }] };
  const finding = { path: "src/lib.rs", line: 4, side: "RIGHT", body: "Handle the empty input", diff_hunk: "@@ -1 +1 @@", skipped: false, published: false };
  const onHandoff = vi.fn();
  vi.mocked(invoke).mockImplementation(async (command: string, args?: unknown) => {
    if (command === "pr_workspace_begin") return "workspace";
    if (command === "pr_triage_begin") return picker;
    if (command === "pr_triage_snapshot") return fixes;
    if (command === "pr_workspace_fix_targets") return fixes.fix_targets;
    if (command === "pr_triage_act") return fixes;
    if (command === "pr_workspace_select_pr" || command === "pr_workspace_switch_ai" || command === "pr_workspace_switch_threads") return undefined;
    if (command === "pr_workspace_prepare_fix") return { target: { ...target, session_id: "session-1" }, draft_prompt: "Handle the empty input" };
    if (command === "pr_workspace_ai_snapshot" || command === "pr_workspace_ai_act") {
      const action = (args as { action?: { kind: string } } | undefined)?.action?.kind;
      if (action === "post") return { workflow_id: "workspace", pr_number: 42, head_sha: "abcdef012345", running: false, stage: null, activity: null, elapsed_seconds: 1, token_estimate: 100, findings: [{ ...finding, published: true }], summary: "Review summary", error: null, precall: null, can_cancel: false };
      return { workflow_id: "workspace", pr_number: 42, head_sha: "abcdef012345", running: false, stage: null, activity: null, elapsed_seconds: 1, token_estimate: 100, findings: [finding], summary: "Review summary", result_revision: "result-rev", error: null, precall: null, can_cancel: false };
    }
    throw new Error(`Unexpected command ${command}`);
  });
  render(<PrWorkspacePanel target={target} onClose={vi.fn()} onReturnToFeature={vi.fn()} onStatusChange={vi.fn()} onHandoff={onHandoff} />);
  fireEvent.click(await screen.findByRole("button", { name: /#42 Add feature/ }));
  fireEvent.click(await screen.findByRole("button", { name: "AI Review" }));
  fireEvent.click(await screen.findByRole("button", { name: "Select all" }));
  expect((await screen.findByRole("checkbox", { name: "Batch" }) as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Clear selection" }));
  expect((screen.getByRole("checkbox", { name: "Batch" }) as HTMLInputElement).checked).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "Select all" }));
  fireEvent.change(screen.getByRole("combobox", { name: "Fix agent" }), { target: { value: "session-1" } });
  fireEvent.click(screen.getByRole("button", { name: "Prepare batch fix (1)" }));
  await waitFor(() => expect(onHandoff).toHaveBeenCalledWith(expect.objectContaining({
    target: { ...target, session_id: "session-1" },
    draft_prompt: expect.stringContaining("Handle the empty input"),
  })));
  expect(await screen.findByRole("button", { name: "Post to GitHub…" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Post to GitHub…" }));
  expect(await screen.findByRole("alertdialog", { name: "Confirm post AI review to GitHub" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Post review to GitHub" }));
  await waitFor(() => expect(vi.mocked(invoke).mock.calls.some(([name, args]) => name === "pr_workspace_ai_act" && (args as { action: { kind: string } }).action.kind === "post")).toBe(true));
  expect(await screen.findByText("Posted")).toBeTruthy();
});

it("opens the feature PR in a shared Files/Threads/AI workspace and cancels a running review", async () => {
  let running = false;
  let picked = false;
  const onReturnToFeature = vi.fn();
  const onStatusChange = vi.fn();
  vi.mocked(invoke).mockImplementation(async (command: string, args?: unknown) => {
    if (command === "pr_workspace_begin") return "workspace";
    if (command === "pr_triage_begin") return picker;
    if (command === "pr_triage_snapshot") return picked ? reviewing : picker;
    if (command === "pr_triage_act") { picked = true; return reviewing; }
    if (command === "pr_workspace_select_pr" || command === "pr_workspace_switch_ai" || command === "pr_workspace_switch_threads" || command === "pr_workspace_close") return undefined;
    if (command === "pr_workspace_fix_targets") return [];
    if (command === "pr_workspace_files") return { workflow_id: "workspace", pr_number: 42, head_sha: "abcdef012345", files: [{ path: "src/lib.rs", old_path: null, status: "modified", additions: 1, deletions: 0, is_binary: false, hunks: [], patch: "", syntax: null }] };
    if (command === "pr_workspace_ai_snapshot" || command === "pr_workspace_ai_act") {
      const kind = (args as { action?: { kind: string } } | undefined)?.action?.kind;
      if (kind === "start") running = true;
      if (kind === "cancel") running = false;
      return { workflow_id: "workspace", pr_number: 42, head_sha: "abcdef012345", running, stage: running ? "Reviewing code" : null, activity: "Reviewing src/lib.rs (initial)", elapsed_seconds: 15, token_estimate: 900, findings: [], summary: null, error: null, precall: null, can_cancel: running };
    }
    throw new Error(`Unexpected command ${command}`);
  });

  render(<PrWorkspacePanel target={target} onClose={vi.fn()} onReturnToFeature={onReturnToFeature} onStatusChange={onStatusChange} />);
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
  fireEvent.click(screen.getByRole("button", { name: "Back to feature" }));
  expect(onReturnToFeature).toHaveBeenCalledOnce();
  expect(onStatusChange).toHaveBeenLastCalledWith({ running: true, prNumber: 42, completed: false });
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

  render(<PrWorkspacePanel target={target} onClose={vi.fn()} onReturnToFeature={vi.fn()} onStatusChange={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: /#42 Add feature/ }));
  await waitFor(() => expect(screen.getByText(/Loading review comments for PR #42/)).toBeTruthy());
  await waitFor(() => expect(snapshots).toBe(1), { timeout: 1500 });
  await new Promise((resolve) => window.setTimeout(resolve, 800));
  expect(snapshots).toBe(1);

  await act(async () => resolveSnapshot(reviewing));
});

it("blocks duplicate fix preparation and ignores a reply after leaving the workspace", async () => {
  const session = { ...target, session_id: "session-1" };
  const fixes: PrTriageView = { ...reviewing, fix_targets: [{ target: session, label: "Claude 1", harness: "claude", stopped: false, destination_revision: "destination-v1" }] };
  const finding = { path: "src/lib.rs", line: 4, side: "RIGHT", body: "A finding", diff_hunk: null, skipped: false, published: false };
  let resolveFix!: (value: unknown) => void;
  const onHandoff = vi.fn();
  vi.mocked(invoke).mockImplementation(async (command: string) => {
    if (command === "pr_workspace_begin") return "workspace";
    if (command === "pr_triage_begin") return picker;
    if (command === "pr_triage_snapshot" || command === "pr_triage_act") return fixes;
    if (command === "pr_workspace_fix_targets") return fixes.fix_targets;
    if (command === "pr_workspace_select_pr" || command === "pr_workspace_switch_ai" || command === "pr_workspace_switch_threads" || command === "pr_workspace_close") return undefined;
    if (command === "pr_workspace_ai_snapshot") return { workflow_id: "workspace", pr_number: 42, head_sha: "abcdef012345", result_revision: "result-v1", running: false, stage: null, activity: null, elapsed_seconds: 0, token_estimate: null, findings: [finding], summary: null, error: null, precall: null, can_cancel: false };
    if (command === "pr_workspace_prepare_fix") return new Promise((resolve) => { resolveFix = resolve; });
    throw new Error(`Unexpected command ${command}`);
  });
  render(<PrWorkspacePanel target={target} onClose={vi.fn()} onReturnToFeature={vi.fn()} onStatusChange={vi.fn()} onHandoff={onHandoff} />);
  fireEvent.click(await screen.findByRole("button", { name: /#42 Add feature/ }));
  fireEvent.click(await screen.findByRole("button", { name: "AI Review" }));
  const aiPanel = await screen.findByRole("region", { name: "AI PR review" });
  fireEvent.change(within(aiPanel).getByRole("combobox", { name: "Fix agent" }), { target: { value: "session-1" } });
  const button = screen.getByRole("button", { name: "Prepare fix" });
  await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(button); fireEvent.click(button);
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "pr_workspace_prepare_fix")).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "Back to feature" }));
  await act(async () => resolveFix({ target: session, draft_prompt: "draft" }));
  expect(onHandoff).not.toHaveBeenCalled();
});

it("retains an unavailable agent choice after refresh instead of silently selecting another", async () => {
  const first = { target: { ...target, session_id: "first" }, label: "First", harness: "claude" as const, stopped: false, destination_revision: "first-v1" };
  const second = { target: { ...target, session_id: "second" }, label: "Second", harness: "codex" as const, stopped: false, destination_revision: "second-v1" };
  const fixes: PrTriageView = { ...reviewing, fix_targets: [first, second] };
  const finding = { path: null, line: null, side: null, body: "Finding", diff_hunk: null, skipped: false, published: false };
  let targets = [first, second];
  vi.mocked(invoke).mockImplementation(async (command: string) => {
    if (command === "pr_workspace_begin") return "workspace";
    if (command === "pr_triage_begin") return picker;
    if (command === "pr_triage_snapshot" || command === "pr_triage_act") return fixes;
    if (command === "pr_workspace_fix_targets") return targets;
    if (command === "pr_workspace_select_pr" || command === "pr_workspace_switch_ai" || command === "pr_workspace_close") return undefined;
    if (command === "pr_workspace_ai_snapshot") return { workflow_id: "workspace", pr_number: 42, head_sha: "abcdef012345", result_revision: "result", running: false, stage: null, activity: null, elapsed_seconds: 0, token_estimate: null, findings: [finding], summary: null, error: null, precall: null, can_cancel: false };
    throw new Error(`Unexpected command ${command}`);
  });
  render(<PrWorkspacePanel target={target} onClose={vi.fn()} onReturnToFeature={vi.fn()} onStatusChange={vi.fn()} onHandoff={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: /#42 Add feature/ }));
  fireEvent.click(await screen.findByRole("button", { name: "AI Review" }));
  const aiPanel = await screen.findByRole("region", { name: "AI PR review" });
  const select = within(aiPanel).getByRole("combobox", { name: "Fix agent" }) as HTMLSelectElement;
  fireEvent.change(select, { target: { value: "first" } });
  targets = [second];
  fireEvent.click(within(aiPanel).getByRole("button", { name: "Refresh agents" }));
  await waitFor(() => expect(within(aiPanel).getByText("Previously selected session unavailable or changed")).toBeTruthy());
  expect(select.value).toBe("__stale");
  fireEvent.click(within(aiPanel).getByRole("button", { name: "Prepare fix" }));
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "pr_workspace_prepare_fix")).toHaveLength(0);
  fireEvent.change(select, { target: { value: "second" } });
  expect(select.value).toBe("second");
});

it("skips, restores and edits a finding against the current result and surfaces refusals", async () => {
  let revision = "rev-1";
  let finding = { path: "src/lib.rs", line: 4, side: "RIGHT", body: "Original body", diff_hunk: null, skipped: false, published: false };
  const view = () => ({ workflow_id: "workspace", pr_number: 42, head_sha: "abcdef012345", result_revision: revision, running: false, stage: null, activity: null, elapsed_seconds: 0, token_estimate: null, findings: [finding], summary: null, error: null, precall: null, can_cancel: false });
  vi.mocked(invoke).mockImplementation(async (command: string, args?: unknown) => {
    if (command === "pr_workspace_begin") return "workspace";
    if (command === "pr_triage_begin") return picker;
    if (command === "pr_triage_snapshot" || command === "pr_triage_act") return reviewing;
    if (command === "pr_workspace_fix_targets") return [];
    if (command === "pr_workspace_select_pr" || command === "pr_workspace_switch_ai" || command === "pr_workspace_close") return undefined;
    if (command === "pr_workspace_ai_snapshot") return view();
    if (command === "pr_workspace_ai_act") {
      const action = (args as { action: { kind: string; index: number; body?: string; result_revision: string; head_sha: string } }).action;
      if (action.result_revision !== revision) throw { kind: "conflict", message: "The AI result changed since you opened it; review the current findings and try again" };
      if (action.kind === "toggle_skip") { finding = { ...finding, skipped: !finding.skipped }; revision = `${revision}+`; }
      if (action.kind === "edit_body") { finding = { ...finding, body: action.body! }; revision = `${revision}+`; }
      return view();
    }
    throw new Error(`Unexpected command ${command}`);
  });
  render(<PrWorkspacePanel target={target} onClose={vi.fn()} onReturnToFeature={vi.fn()} onStatusChange={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: /#42 Add feature/ }));
  fireEvent.click(await screen.findByRole("button", { name: "AI Review" }));

  fireEvent.click(await screen.findByRole("button", { name: "Edit finding 1" }));
  const box = screen.getByRole("textbox", { name: "Finding 1 body" }) as HTMLTextAreaElement;
  fireEvent.change(box, { target: { value: "x".repeat(8001) } });
  expect((screen.getByRole("button", { name: "Save body" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(box, { target: { value: "  " } });
  expect((screen.getByRole("button", { name: "Save body" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(box, { target: { value: "Edited body" } });
  fireEvent.click(screen.getByRole("button", { name: "Save body" }));
  await screen.findByText("Edited body");
  const editCall = vi.mocked(invoke).mock.calls.find(([, a]) => (a as { action?: { kind: string } })?.action?.kind === "edit_body")!;
  expect(editCall[1]).toMatchObject({ action: { kind: "edit_body", index: 0, body: "Edited body", result_revision: "rev-1", head_sha: "abcdef012345" } });
  expect(screen.queryByRole("textbox", { name: "Finding 1 body" })).toBeNull();

  fireEvent.click(await screen.findByRole("button", { name: "Skip finding 1" }));
  await screen.findByText("Skipped");
  expect(screen.queryByRole("button", { name: "Post to GitHub…" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Restore finding 1" }));
  await screen.findByRole("button", { name: "Skip finding 1" });
  expect(screen.getByRole("button", { name: "Post to GitHub…" })).toBeTruthy();

  // A result that moved on elsewhere is refused loudly and nothing is retargeted.
  revision = "rev-other";
  const before = finding.body;
  fireEvent.click(screen.getByRole("button", { name: "Skip finding 1" }));
  expect(await screen.findByText(/AI result changed/)).toBeTruthy();
  expect(finding.body).toBe(before);
  expect(finding.skipped).toBe(false);
});

it("authors review comments in the Files tab and leaves them saved when switching tabs", async () => {
  const calls: string[] = [];
  const line = (kind: "context" | "added", old_line: number | null, new_line: number | null, text: string) => ({ kind, old_line, new_line, text });
  const review = {
    workflow_id: "files-review", revision: 1, project_name: "Demo", head_sha: "abcdef012345", stage: "review" as const, embedded: true, pin_error: null, loading: false,
    entries: [], number: 42, title: "Add feature", summary: "", submission: null, error: null, notice: null,
    files: [{ diff: { path: "src/lib.rs", old_path: null, status: "modified", additions: 1, deletions: 0, is_binary: false, patch: "", hunks: [{ header: "@@ -4,1 +4,2 @@", lines: [line("context", 4, 4, " four"), line("added", null, 5, "+five")] }] },
      comment: "", line_comments: [], pr_lines: [{ old_line: 4, new_line: 4, hunk: 0 }, { old_line: null, new_line: 5, hunk: 0 }] }],
  };
  vi.mocked(invoke).mockImplementation(async (command: string) => {
    calls.push(command);
    if (command === "pr_workspace_begin") return "workspace";
    if (command === "pr_triage_begin") return picker;
    if (command === "pr_triage_snapshot") return reviewing;
    if (command === "pr_triage_act") return reviewing;
    if (command === "pr_workspace_review_open") return review;
    if (command === "pr_workspace_select_pr" || command === "pr_workspace_switch_threads" || command === "pr_workspace_close") return undefined;
    throw new Error(`Unexpected command ${command}`);
  });
  render(<PrWorkspacePanel target={target} onClose={vi.fn()} onReturnToFeature={vi.fn()} onStatusChange={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: /#42 Add feature/ }));
  await screen.findByLabelText("Sort");
  fireEvent.click(screen.getByRole("button", { name: "Files" }));
  expect(await screen.findByRole("region", { name: "PR Files review" })).toBeTruthy();
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("pr_workspace_review_open", { workflowId: "workspace", number: 42 });
  expect(calls).not.toContain("pr_workspace_files");
  // An unsaved edit holds the tab: switching would discard it.
  fireEvent.click(screen.getByRole("button", { name: "Select line 5" }));
  fireEvent.click(screen.getByRole("button", { name: "Comment on selection" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Line comment" }), { target: { value: "Draft" } });
  await waitFor(() => expect((screen.getByRole("button", { name: "Threads" }) as HTMLButtonElement).disabled).toBe(true));
  expect((screen.getByRole("button", { name: "Close workspace" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Cancel edit" }));
  await waitFor(() => expect((screen.getByRole("button", { name: "Threads" }) as HTMLButtonElement).disabled).toBe(false));
  const before = calls.filter((c) => c === "pr_workspace_switch_threads").length;
  fireEvent.click(screen.getByRole("button", { name: "Threads" }));
  await waitFor(() => expect(calls.filter((c) => c === "pr_workspace_switch_threads").length).toBe(before + 1));
  await waitFor(() => expect(screen.queryByRole("region", { name: "PR Files review" })).toBeNull());
});
