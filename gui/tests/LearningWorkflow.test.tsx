// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import App from "../src/App";
import type { LearningView, WorkspaceSnapshot } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));
vi.mock("../src/TerminalPane", async () => {
  const { useEffect } = await import("react");
  return { default: ({ onReadyChange }: { onReadyChange: (ready: boolean) => void }) => {
    useEffect(() => { onReadyChange(true); }, [onReadyChange]);
    return null;
  } };
});
vi.mock("../src/TodoPanel", () => ({ default: () => null }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

it.each([false, true])("gates the Learning handoff and preserves an existing unsent draft (%s)", async (existingDraft) => {
  const snapshot: WorkspaceSnapshot = {
    projects: [{ id: "project", name: "demo", repo: "/demo", is_git: false,
      features: [{ id: "feature", name: "my-feat", branch: "my-feat", workdir: "/demo", is_worktree: false,
        status: "idle", agent: "claude", mode: "vibeless", sessions: [{ id: "shell", label: "Shell", kind: "terminal", tmux_window: "shell" }] }] }],
    snapshot_at: "2026-10-01T00:00:00Z", stopped_session_ids: [],
  };
  if (existingDraft) snapshot.projects[0].features[0].sessions.unshift({
    id: "agent", label: "Learning agent", kind: "claude", tmux_window: "claude",
  });
  const learning: LearningView = {
    workflow_id: "workflow", revision: 3, target: { project_id: "project", feature_id: "feature" }, feature_name: "my-feat",
    scope: "repo_tree", is_git: false, entries: [], content_path: null, content: [], content_line_labels: [], content_error: null,
    anchor: "this whole project", selection: null, hunks: [], starters: [], can_keep_todo: true, harness: "claude", harnesses: ["claude"], level: "newcomer", history_saved: true,
    error: null, notice: null, qa: [{ id: "answer", parent_id: null, question: "Explain the project", answer: "An answer", anchor: "this whole project",
      status: "answered", intent: "explain", harness: "claude", run_mode: "this file only", error: null, drift: null, spawned_session_id: null,
      todo_id: null, todo_seed: null }],
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "get_snapshot": return snapshot;
      case "supported_harnesses": return [{ slug: "claude", display_name: "Claude" }];
      case "supported_modes": return [{ slug: "vibeless", display_name: "Vibeless", description: "" }];
      case "plan_snapshot": return { active: null, draft: null };
      case "learning_begin": return learning;
      case "learning_snapshot": return learning;
      case "learning_launch_agent": {
        if (!(args as { approved: boolean }).approved) throw { kind: "needs_approval", message: "Too many agents" };
        if (!existingDraft) snapshot.projects[0].features[0].sessions.push({ id: "agent", label: "Learning agent", kind: "claude", tmux_window: "claude" });
        return { target: { project_id: "project", feature_id: "feature", session_id: "agent" }, draft_prompt: "Editable learning seed",
          notice: null, info: null, start_required: false };
      }
      default: throw new Error(`Unexpected command: ${command}`);
    }
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><App /></QueryClientProvider>);
  fireEvent.click(await screen.findByRole("button", { name: "my-feat", exact: true }));
  if (existingDraft) fireEvent.change(screen.getByRole("textbox", { name: "Draft prompt" }), {
    target: { value: "My existing message" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Learning", exact: true }));
  await screen.findByRole("dialog", { name: "Learning", exact: true });
  fireEvent.click(screen.getByRole("button", { name: "Open editing agent" }));
  const approval = await screen.findByRole("dialog", { name: "Approve Learning agent" });
  if (existingDraft) expect((screen.getByRole("textbox", { name: "Draft prompt" }) as HTMLTextAreaElement).value).toBe("My existing message");
  else expect(screen.queryByRole("textbox", { name: "Draft prompt" })).toBeNull();
  fireEvent.click(within(approval).getByRole("button", { name: "Cancel" }));
  expect(screen.getByRole("dialog", { name: "Learning", exact: true })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Open editing agent" }));
  const retry = await screen.findByRole("dialog", { name: "Approve Learning agent" });
  fireEvent.click(within(retry).getByRole("button", { name: "Start anyway" }));
  const draft = await screen.findByRole("textbox", { name: "Draft prompt" }) as HTMLTextAreaElement;
  await waitFor(() => expect(draft.value).toBe(existingDraft
    ? "My existing message\n\nEditable learning seed" : "Editable learning seed"));
  expect(document.activeElement).toBe(draft);
  expect(screen.queryByRole("dialog", { name: "Learning", exact: true })).toBeNull();
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "terminal_submit_prompt")).toBe(false);
  client.clear();
});

it("starts a stopped linked editing session through the tab's own start instead of opening another", async () => {
  const snapshot: WorkspaceSnapshot = {
    projects: [{ id: "project", name: "demo", repo: "/demo", is_git: false,
      features: [{ id: "feature", name: "my-feat", branch: "my-feat", workdir: "/demo", is_worktree: false,
        status: "stopped", agent: "claude", mode: "vibeless",
        sessions: [{ id: "agent", label: "Learning agent", kind: "claude", tmux_window: "claude" }] }] }],
    snapshot_at: "2026-10-01T00:00:00Z", stopped_session_ids: ["agent"],
  };
  const learning: LearningView = {
    workflow_id: "workflow", revision: 3, target: { project_id: "project", feature_id: "feature" }, feature_name: "my-feat",
    scope: "repo_tree", is_git: false, entries: [], content_path: null, content: [], content_line_labels: [], content_error: null,
    anchor: "this whole project", selection: null, hunks: [], starters: [], can_keep_todo: true, harness: "claude",
    harnesses: ["claude"], level: "newcomer", history_saved: true, error: null, notice: null,
    qa: [{ id: "answer", parent_id: null, question: "Explain the project", answer: "An answer", anchor: "this whole project",
      status: "answered", intent: "explain", harness: "claude", run_mode: "this file only", error: null, drift: null,
      spawned_session_id: "agent", todo_id: null, todo_seed: null }],
  };
  const target = { project_id: "project", feature_id: "feature", session_id: "agent" };
  vi.mocked(invoke).mockImplementation(async (command) => {
    switch (command) {
      case "get_snapshot": return snapshot;
      case "supported_harnesses": return [{ slug: "claude", display_name: "Claude" }];
      case "supported_modes": return [{ slug: "vibeless", display_name: "Vibeless", description: "" }];
      case "plan_snapshot": return { active: null, draft: null };
      case "learning_begin": return learning;
      case "learning_snapshot": return learning;
      case "learning_launch_agent":
        return { target, draft_prompt: "Editable learning seed", notice: null,
          info: "'Learning agent' is stopped; start it to continue that conversation.", start_required: true };
      case "session_recovery_option": return null;
      case "start_session": return { session_id: "agent", already_running: false, message: "Started" };
      default: throw new Error(`Unexpected command: ${command}`);
    }
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><App /></QueryClientProvider>);
  fireEvent.click(await screen.findByRole("button", { name: "my-feat", exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "Learning", exact: true }));
  await screen.findByRole("dialog", { name: "Learning", exact: true });
  fireEvent.click(screen.getByRole("button", { name: "Return to editing agent" }));
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("start_session", { target, approved: false }));
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("session_recovery_option", { target });
  expect(await screen.findByText(/is stopped; start it/)).toBeTruthy();
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "add_session")).toBe(false);
  client.clear();
});

it("keeps only a handoff's seed when it lands while the earlier draft is still sending", async () => {
  const snapshot: WorkspaceSnapshot = {
    projects: [{ id: "project", name: "demo", repo: "/demo", is_git: false,
      features: [{ id: "feature", name: "my-feat", branch: "my-feat", workdir: "/demo", is_worktree: false,
        status: "idle", agent: "claude", mode: "vibeless",
        sessions: [{ id: "agent", label: "Learning agent", kind: "claude", tmux_window: "claude" }] }] }],
    snapshot_at: "2026-10-01T00:00:00Z", stopped_session_ids: [],
  };
  const learning: LearningView = {
    workflow_id: "workflow", revision: 3, target: { project_id: "project", feature_id: "feature" }, feature_name: "my-feat",
    scope: "repo_tree", is_git: false, entries: [], content_path: null, content: [], content_line_labels: [], content_error: null,
    anchor: "this whole project", selection: null, hunks: [], starters: [], can_keep_todo: true, harness: "claude",
    harnesses: ["claude"], level: "newcomer", history_saved: true, error: null, notice: null,
    qa: [{ id: "answer", parent_id: null, question: "Explain the project", answer: "An answer", anchor: "this whole project",
      status: "answered", intent: "explain", harness: "claude", run_mode: "this file only", error: null, drift: null,
      spawned_session_id: null, todo_id: null, todo_seed: null }],
  };
  const target = { project_id: "project", feature_id: "feature", session_id: "agent" };
  let finishSend!: () => void;
  vi.mocked(invoke).mockImplementation(async (command) => {
    switch (command) {
      case "get_snapshot": return snapshot;
      case "supported_harnesses": return [{ slug: "claude", display_name: "Claude" }];
      case "supported_modes": return [{ slug: "vibeless", display_name: "Vibeless", description: "" }];
      case "plan_snapshot": return { active: null, draft: null };
      case "learning_begin": return learning;
      case "learning_snapshot": return learning;
      case "learning_launch_agent":
        return { target, draft_prompt: "Editable learning seed", notice: null, info: null, start_required: false };
      case "terminal_submit_prompt": return new Promise<void>((resolve) => { finishSend = resolve; });
      default: throw new Error(`Unexpected command: ${command}`);
    }
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><App /></QueryClientProvider>);
  fireEvent.click(await screen.findByRole("button", { name: "my-feat", exact: true }));
  const draft = () => screen.getByRole("textbox", { name: "Draft prompt" }) as HTMLTextAreaElement;
  fireEvent.change(draft(), { target: { value: "Already delivered" } });
  fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
  await waitFor(() => expect(finishSend).toBeDefined());
  fireEvent.click(screen.getByRole("button", { name: "Learning", exact: true }));
  await screen.findByRole("dialog", { name: "Learning", exact: true });
  fireEvent.click(screen.getByRole("button", { name: "Open editing agent" }));
  await waitFor(() => expect(draft().value).toBe("Already delivered\n\nEditable learning seed"));
  await act(async () => finishSend());
  await waitFor(() => expect(draft().value).toBe("Editable learning seed"));
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "terminal_submit_prompt")).toEqual([
    ["terminal_submit_prompt", { key: "feature:agent", text: "Already delivered" }],
  ]);
  client.clear();
});
