// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import App from "../src/App";
import type { Feature, FeatureSession, WorkspaceSnapshot } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));
const terminal = vi.hoisted(() => ({ ready: true }));
vi.mock("../src/TerminalPane", async () => {
  const { useEffect } = await import("react");
  return { default: ({ onReadyChange }: { onReadyChange: (ready: boolean) => void }) => {
    useEffect(() => { onReadyChange(terminal.ready); }, [onReadyChange]);
    return <button onClick={() => onReadyChange(true)}>Connect terminal</button>;
  } };
});
vi.mock("../src/TodoPanel", () => ({ default: () => null }));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  terminal.ready = true;
});

function session(id: string, kind = "terminal"): FeatureSession {
  return { id, kind, label: id, tmux_window: id };
}

const promptCalls = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "terminal_submit_prompt");
const draftInput = () => screen.getByRole("textbox", { name: "Draft prompt" }) as HTMLTextAreaElement;

it.each(["claude", "codex", "opencode", "pi"])("composes locally for %s and stays available after sending", async (kind) => {
  const client = await openFeature([session("Agent", kind)]);
  const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((command, args, options) =>
    command === "terminal_submit_prompt" ? Promise.resolve() : original(command, args, options));
  expect(document.activeElement).toBe(draftInput());
  const text = "Please fix this.\n\nKeep Unicode: café 世界 🚀";
  fireEvent.change(draftInput(), { target: { value: text } });
  fireEvent.keyDown(draftInput(), { key: "Enter" });
  fireEvent.keyDown(draftInput(), { key: "Enter", shiftKey: true });
  expect(promptCalls()).toHaveLength(0);
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "terminal_input")).toBe(false);
  fireEvent.keyDown(draftInput(), { key: "Enter", ctrlKey: true });
  await waitFor(() => expect(draftInput().value).toBe(""));
  expect(promptCalls()).toEqual([["terminal_submit_prompt", { key: "feature:Agent", text }]]);
  fireEvent.change(draftInput(), { target: { value: "Next message" } });
  fireEvent.keyDown(draftInput(), { key: "Enter", metaKey: true });
  await waitFor(() => expect(promptCalls()).toHaveLength(2));
  client.clear();
});

it("retains separate drafts across tabs, TODOs, workspace navigation and snapshot refreshes", async () => {
  const client = await openFeature([session("First", "claude"), session("Second", "codex")]);
  fireEvent.change(draftInput(), { target: { value: "First draft" } });
  fireEvent.click(screen.getByRole("tab", { name: /Second/ }));
  expect(draftInput().value).toBe("");
  fireEvent.change(draftInput(), { target: { value: "Second draft" } });
  fireEvent.click(screen.getByRole("tab", { name: "TODOs" }));
  expect(screen.queryByRole("textbox", { name: "Draft prompt" })).toBeNull();
  fireEvent.click(screen.getByRole("tab", { name: /First/ }));
  expect(draftInput().value).toBe("First draft");
  fireEvent.click(screen.getByRole("button", { name: "demo", exact: true }));
  expect(screen.queryByRole("textbox", { name: "Draft prompt" })).toBeNull();
  await act(async () => { await client.invalidateQueries({ queryKey: ["workspace-snapshot"] }); });
  fireEvent.click(screen.getByRole("button", { name: "my-feat", exact: true }));
  expect(draftInput().value).toBe("First draft");
  fireEvent.click(screen.getByRole("button", { name: "Clear", exact: true }));
  expect(draftInput().value).toBe("");
  fireEvent.click(screen.getByRole("tab", { name: /Second/ }));
  expect(draftInput().value).toBe("Second draft");
  expect(promptCalls()).toHaveLength(0);
  client.clear();
});

it("locks a pending send, retains failed drafts and allows retry", async () => {
  const client = await openFeature([session("Agent", "claude")]);
  const original = vi.mocked(invoke).getMockImplementation()!;
  let rejectSend!: (error: unknown) => void;
  vi.mocked(invoke).mockImplementation((command, args, options) => command === "terminal_submit_prompt"
    ? new Promise((_, reject) => { rejectSend = reject; }) : original(command, args, options));
  fireEvent.change(draftInput(), { target: { value: "Keep on failure" } });
  const send = screen.getByRole("button", { name: "Send prompt" });
  fireEvent.click(send);
  fireEvent.click(send);
  fireEvent.keyDown(draftInput(), { key: "Enter", ctrlKey: true });
  fireEvent.change(draftInput(), { target: { value: "An edit during send" } });
  expect(promptCalls()).toHaveLength(1);
  expect(draftInput().readOnly).toBe(true);
  expect(draftInput().value).toBe("Keep on failure");
  expect((screen.getByRole("button", { name: "Clear" }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => rejectSend({ kind: "not_found", message: "Terminal disconnected" }));
  expect(await screen.findByText("Terminal disconnected")).toBeTruthy();
  expect(draftInput().value).toBe("Keep on failure");
  expect(draftInput().readOnly).toBe(false);
  vi.mocked(invoke).mockImplementation((command, args, options) =>
    command === "terminal_submit_prompt" ? Promise.resolve() : original(command, args, options));
  fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
  await waitFor(() => expect(draftInput().value).toBe(""));
  expect(promptCalls()).toHaveLength(2);
  client.clear();
});

it("clears only the originating draft when a send completes after switching sessions", async () => {
  const client = await openFeature([session("First", "claude"), session("Second", "pi")]);
  const original = vi.mocked(invoke).getMockImplementation()!;
  let finishSend!: () => void;
  vi.mocked(invoke).mockImplementation((command, args, options) => command === "terminal_submit_prompt"
    ? new Promise<void>((resolve) => { finishSend = resolve; }) : original(command, args, options));
  fireEvent.change(draftInput(), { target: { value: "First message" } });
  fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
  fireEvent.click(screen.getByRole("tab", { name: /Second/ }));
  fireEvent.change(draftInput(), { target: { value: "Second message" } });
  expect(draftInput().readOnly).toBe(false);
  await act(async () => finishSend());
  expect(draftInput().value).toBe("Second message");
  fireEvent.click(screen.getByRole("tab", { name: /First/ }));
  expect(draftInput().value).toBe("");
  expect(promptCalls()).toEqual([["terminal_submit_prompt", { key: "feature:First", text: "First message" }]]);
  client.clear();
});

it("blocks empty prompts, unconnected sends and IME confirmation", async () => {
  terminal.ready = false;
  const client = await openFeature([session("Agent", "claude")]);
  fireEvent.change(draftInput(), { target: { value: "A message" } });
  fireEvent.keyDown(draftInput(), { key: "Enter", ctrlKey: true });
  expect(promptCalls()).toHaveLength(0);
  expect((screen.getByRole("button", { name: "Send prompt" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Connect terminal" }));
  fireEvent.keyDown(draftInput(), { key: "Enter", ctrlKey: true, isComposing: true });
  fireEvent.keyDown(draftInput(), { key: "Enter", metaKey: true, keyCode: 229 });
  expect(promptCalls()).toHaveLength(0);
  fireEvent.change(draftInput(), { target: { value: " \n\t" } });
  fireEvent.keyDown(draftInput(), { key: "Enter", ctrlKey: true });
  expect(promptCalls()).toHaveLength(0);
  expect((screen.getByRole("button", { name: "Send prompt" }) as HTMLButtonElement).disabled).toBe(true);
  client.clear();
});

it.each(["terminal", "nvim", "custom"])("keeps %s tabs using their terminal input", async (kind) => {
  const client = await openFeature([session("Direct", kind)]);
  expect(screen.queryByRole("textbox", { name: "Draft prompt" })).toBeNull();
  client.clear();
});

it("hides the composer on a stopped agent and restores its draft when restarted", async () => {
  const client = await openFeature([session("Agent", "claude")]);
  fireEvent.change(draftInput(), { target: { value: "After restart" } });
  const snapshot = client.getQueryData<WorkspaceSnapshot>(["workspace-snapshot"])!;
  act(() => client.setQueryData(["workspace-snapshot"], { ...snapshot, stopped_session_ids: ["Agent"] }));
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "Draft prompt" })).toBeNull());
  act(() => client.setQueryData(["workspace-snapshot"], snapshot));
  await waitFor(() => expect(draftInput().value).toBe("After restart"));
  expect(promptCalls()).toHaveLength(0);
  client.clear();
});

async function openFeature(
  sessions: FeatureSession[],
  stoppedSessionIds: string[] = [],
  status: Feature["status"] = "idle",
  isGit = false,
) {
  const snapshot: WorkspaceSnapshot = {
    projects: [{
      id: "project", name: "demo", repo: "/demo", is_git: isGit,
      features: [{
        id: "feature", name: "my-feat", branch: "my-feat", workdir: "/demo",
        is_worktree: false, status, agent: "claude", mode: "vibeless", sessions,
      }],
    }],
    snapshot_at: "2026-09-26T00:00:00Z",
    stopped_session_ids: stoppedSessionIds,
  };
  vi.mocked(invoke).mockImplementation(async (command) => {
    switch (command) {
      case "get_snapshot": return snapshot;
      case "supported_harnesses": return [{ slug: "claude", display_name: "Claude" }];
      case "supported_modes": return [{ slug: "vibeless", display_name: "Vibeless", description: "" }];
      case "plan_snapshot": return { active: null, draft: null };
      default: throw new Error(`Unexpected command: ${command}`);
    }
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><App /></QueryClientProvider>);
  fireEvent.click(await screen.findByRole("button", { name: "my-feat", exact: true }));
  return client;
}

it("retains the worktree disposition through the project-list choice and submits stable IDs once", async () => {
  const client = await openFeature([session("Shell")], [], "stopped");
  const initialInvoke = vi.mocked(invoke).getMockImplementation()!;
  let deleteCalls = 0;
  let finishDeletion!: () => void;
  const deleting = new Promise((resolve) => {
    finishDeletion = () => resolve({ status: "deleted", feature_id: "feature", message: "List kept" });
  });
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "delete_feature") {
      deleteCalls++;
      if (deleteCalls === 1) return Promise.resolve({ status: "needs_todo_disposition", unfinished: 2 });
      if (deleteCalls === 2) return Promise.resolve({ status: "needs_todo_host", prompt: {
        list_id: "project-list", todo_count: 3,
        candidates: [{ feature_id: "first", name: "First" }, { feature_id: "second", name: "Second" }],
      } });
      return deleting;
    }
    return initialInvoke(command, args, options);
  });
  fireEvent.click(screen.getByRole("button", { name: "More feature actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Delete feature" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete feature", exact: true }));
  fireEvent.click(await screen.findByRole("radio", { name: "Move them to the global list" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete feature", exact: true }));
  fireEvent.click(await screen.findByRole("radio", { name: "Keep on Second" }));
  const button = screen.getByRole("button", { name: "Delete feature", exact: true });
  fireEvent.click(button);
  fireEvent.click(button);
  await waitFor(() => expect(deleteCalls).toBe(3));
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("delete_feature", {
    target: { project_id: "project", feature_id: "feature" },
    todos: "move_to_global", todoHost: { list_id: "project-list", feature_id: "second" },
  });
  await act(async () => { finishDeletion(); });
  expect(screen.queryByRole("dialog", { name: "Delete feature" })).toBeNull();
  expect(await screen.findByText("List kept")).toBeTruthy();
  client.clear();
});

it("cancels a project-list choice without issuing another deletion request", async () => {
  const client = await openFeature([session("Shell")], [], "stopped");
  const initialInvoke = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "delete_feature") return Promise.resolve({ status: "needs_todo_host", prompt: {
      list_id: "project-list", todo_count: 3, candidates: [{ feature_id: "first", name: "First" }],
    } });
    return initialInvoke(command, args, options);
  });
  fireEvent.click(screen.getByRole("button", { name: "More feature actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Delete feature" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete feature", exact: true }));
  await screen.findByRole("radio", { name: "Keep on First" });
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("dialog", { name: "Delete feature" })).toBeNull();
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "delete_feature")).toHaveLength(1);
  client.clear();
});

it("opens current changes from a stopped Git feature without launching a session", async () => {
  const client = await openFeature([], [], "stopped", true);
  const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "load_diff") return Promise.resolve({
      target: { project_id: "project", feature_id: "feature" }, feature_name: "my-feat",
      branch: "my-feat", base_ref: "main", base_commit: "12345678", commit: null,
      commits: [], commits_error: null, files: [], total_additions: 0, total_deletions: 0,
    });
    return original(command, args, options);
  });
  fireEvent.click(screen.getByRole("button", { name: "Changes", exact: true }));
  await screen.findByText("No changes in this scope.");
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("load_diff", {
    target: { project_id: "project", feature_id: "feature" },
    options: { commit: null, base_ref: null, ignore_whitespace: false, context: "standard" },
  });
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "start_feature" || command === "add_session")).toBe(false);
  fireEvent.click(screen.getAllByRole("button", { name: "Close", exact: true })[0]);
  expect(screen.queryByRole("dialog", { name: "Diff viewer" })).toBeNull();
  client.clear();
});

it("offers Changes only for Git projects", async () => {
  const client = await openFeature([], [], "stopped");
  expect(screen.queryByRole("button", { name: "Changes", exact: true })).toBeNull();
  expect(screen.queryByRole("button", { name: "Final Review", exact: true })).toBeNull();
  client.clear();
});

it("opens Final Review on a stopped feature and pauses with its workflow identity once", async () => {
  const client = await openFeature([], [], "stopped", true);
  const original = vi.mocked(invoke).getMockImplementation()!;
  let finishPause!: () => void;
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "review_begin") return Promise.resolve({
      workflow_id: "review-id", revision: 7, target: { project_id: "project", feature_id: "feature" },
      feature_name: "my-feat", branch: "my-feat", base_ref: "main", files: [], selected_path: null,
        general_feedback: "", has_prior_review: false, error: null, save_error: null, applied_suggestions: [],
    });
    if (command === "review_act") return new Promise((resolve) => { finishPause = () => resolve(null); });
    return original(command, args, options);
  });
  fireEvent.click(screen.getByRole("button", { name: "Final Review", exact: true }));
  await screen.findByText("No changes to review.");
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("review_begin", { target: { project_id: "project", feature_id: "feature" } });
  const pause = screen.getByRole("button", { name: "Pause review" });
  fireEvent.click(pause);
  fireEvent.click(pause);
  await waitFor(() => expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "review_act")).toHaveLength(1));
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("review_act", { workflowId: "review-id", revision: 7, action: { kind: "pause" } });
  await act(async () => finishPause());
  expect(screen.queryByRole("dialog", { name: "Final Review" })).toBeNull();
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "start_feature" || command === "add_session")).toBe(false);
  client.clear();
});
