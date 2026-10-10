// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  localStorage.clear();
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
  // Opening a tab must not pull keyboard focus away from the terminal.
  expect(document.activeElement).not.toBe(draftInput());
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

it("drops a closed session's draft so a reused session id starts empty", async () => {
  const client = await openFeature([session("First", "claude"), session("Second", "codex")]);
  fireEvent.click(screen.getByRole("tab", { name: /Second/ }));
  fireEvent.change(draftInput(), { target: { value: "Unsent" } });
  const snapshot = client.getQueryData<WorkspaceSnapshot>(["workspace-snapshot"])!;
  const feature = snapshot.projects[0].features[0];
  const without = (sessions: FeatureSession[]): WorkspaceSnapshot => ({
    ...snapshot, projects: [{ ...snapshot.projects[0], features: [{ ...feature, sessions }] }],
  });
  act(() => client.setQueryData(["workspace-snapshot"], without([feature.sessions[0]])));
  await waitFor(() => expect(screen.queryByRole("tab", { name: /Second/ })).toBeNull());
  act(() => client.setQueryData(["workspace-snapshot"], without(feature.sessions)));
  fireEvent.click(await screen.findByRole("tab", { name: /Second/ }));
  expect(draftInput().value).toBe("");
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

it("keeps a stopped agent draft editable with sending disabled and restores it when restarted", async () => {
  const client = await openFeature([session("Agent", "claude")]);
  fireEvent.change(draftInput(), { target: { value: "After restart" } });
  const snapshot = client.getQueryData<WorkspaceSnapshot>(["workspace-snapshot"])!;
  act(() => client.setQueryData(["workspace-snapshot"], { ...snapshot, stopped_session_ids: ["Agent"] }));
  await waitFor(() => expect(screen.getByText("Start this session to send your draft.")).toBeTruthy());
  expect(draftInput().value).toBe("After restart");
  expect((screen.getByRole("button", { name: "Send prompt" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(draftInput(), { target: { value: "Edited while stopped" } });
  act(() => client.setQueryData(["workspace-snapshot"], snapshot));
  await waitFor(() => expect(draftInput().value).toBe("Edited while stopped"));
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
  expect(screen.queryByRole("button", { name: "PR Triage", exact: true })).toBeNull();
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
        general_feedback: "", has_prior_review: false, error: null, save_error: null, applied_suggestions: [], history: null, summary: null,
        ai: { precall: null, running: false, walkthrough_path: null, co_review_path: null, overview_running: false, overview: null, question_running: false, questions: [], question_error: null, comment_draft: null, ready_comment: null, harnesses: ["claude"], message: null },
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

it("polls review completions by workflow/revision and cannot resurrect a paused review", async () => {
  const client = await openFeature([], [], "stopped", true);
  const original = vi.mocked(invoke).getMockImplementation()!;
  const base = {
    workflow_id: "review-poll", revision: 7, target: { project_id: "project", feature_id: "feature" },
    feature_name: "my-feat", branch: "my-feat", base_ref: "main", files: [], selected_path: null,
    general_feedback: "", has_prior_review: false, error: null, save_error: null, applied_suggestions: [], history: null, summary: null,
    ai: { precall: null, running: true, walkthrough_path: null, co_review_path: null, overview_running: false, overview: null as string | null, question_running: true, questions: [], question_error: null, comment_draft: null, ready_comment: null, harnesses: ["claude"], message: null },
  };
  // The overview landed while a question is still running, so polling continues.
  let response: Promise<typeof base> = Promise.resolve({ ...base, revision: 8, ai: { ...base.ai, overview: "Completed overview" } });
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "review_begin") return Promise.resolve(base);
    if (command === "review_snapshot") return response;
    if (command === "review_act") return Promise.resolve(null);
    return original(command, args, options);
  });
  vi.useFakeTimers();
  try {
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Final Review", exact: true })));
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    expect(screen.getByText("Completed overview")).toBeTruthy();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("review_snapshot", { workflowId: "review-poll" });
    response = Promise.resolve({ ...base, revision: 7, ai: { ...base.ai, overview: "Stale overview" } });
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    expect(screen.queryByText("Stale overview")).toBeNull();
    let deliver!: (next: typeof base) => void;
    response = new Promise((resolve) => { deliver = resolve; });
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Pause review" })));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Discard and continue" })));
    await act(async () => deliver({ ...base, revision: 9 }));
    expect(screen.queryByRole("dialog", { name: "Final Review" })).toBeNull();
    const mutations = vi.mocked(invoke).mock.calls.filter(([command]) => command === "review_act");
    expect(mutations).toHaveLength(1);
    expect(mutations[0][1]).toEqual({ workflowId: "review-poll", revision: 8, action: { kind: "pause" } });
  } finally { vi.useRealTimers(); client.clear(); }
});

it("does not poll a review with no AI work in flight", async () => {
  const client = await openFeature([], [], "stopped", true);
  const original = vi.mocked(invoke).getMockImplementation()!;
  const idle = {
    workflow_id: "review-idle", revision: 1, target: { project_id: "project", feature_id: "feature" },
    feature_name: "my-feat", branch: "my-feat", base_ref: "main", files: [], selected_path: null,
    general_feedback: "", has_prior_review: false, error: null, save_error: null, applied_suggestions: [], history: null, summary: null,
    ai: { precall: null, running: false, walkthrough_path: null, co_review_path: null, overview_running: false, overview: null, question_running: false, questions: [], question_error: null, comment_draft: null, ready_comment: null, harnesses: ["claude"], message: null },
  };
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "review_begin") return Promise.resolve(idle);
    if (command === "review_snapshot") return Promise.resolve(idle);
    return original(command, args, options);
  });
  vi.useFakeTimers();
  try {
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Final Review", exact: true })));
    await act(async () => vi.advanceTimersByTimeAsync(5000));
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "review_snapshot")).toHaveLength(0);
  } finally { vi.useRealTimers(); client.clear(); }
});

it.each(["claude", "codex", "opencode", "pi"])("adds a library prompt to an existing %s draft without sending or replacing another draft", async (kind) => {
  const client = await openFeature([session("First", kind), session("Second", kind)]);
  fireEvent.change(draftInput(), { target: { value: "First message" } });
  fireEvent.click(screen.getByRole("tab", { name: /Second/ }));
  fireEvent.change(draftInput(), { target: { value: "Second message" } });
  fireEvent.click(screen.getByRole("tab", { name: /First/ }));
  const original = vi.mocked(invoke).getMockImplementation()!;
  const target = { project_id: "project", feature_id: "feature", session_id: "Second" };
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "prompt_library_load") return Promise.resolve({
      entries: [{ key: "plain", name: "Library fixture", source: "User", body: "Library prompt", tags: [], description: null, slots: [] }],
      available_keys: ["plain"],
      targets: ["First", "Second"].map((id) => ({ target: { ...target, session_id: id }, label: id, stopped: false })),
    });
    if (command === "prompt_library_resolve") return Promise.resolve("Library prompt");
    return original(command, args, options);
  });
  fireEvent.click(within(draftInput().closest(".composer")!).getByRole("button", { name: "Prompt library" }));
  fireEvent.click(await screen.findByRole("button", { name: /Library fixture/ }));
  await screen.findByText("Library prompt", { selector: "pre[aria-label='Resolved prompt']" });
  fireEvent.change(screen.getByRole("combobox", { name: "Agent session" }), {
    target: { value: JSON.stringify(["project", "feature", "Second"]) },
  });
  await waitFor(() => expect((screen.getByRole("button", { name: "Add to draft" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Add to draft" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Prompt library" })).toBeNull());
  expect(draftInput().value).toBe("Second message\n\nLibrary prompt");
  expect(document.activeElement).toBe(draftInput());
  fireEvent.click(screen.getByRole("tab", { name: /First/ }));
  expect(draftInput().value).toBe("First message");
  expect(promptCalls()).toHaveLength(0);
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("prompt_library_load", {
    scope: { kind: "feature", project_id: "project", feature_id: "feature" }, query: "",
  });
  client.clear();
});

it("refuses a late library handoff after the session is deleted in the workspace snapshot", async () => {
  const client = await openFeature([session("Agent", "claude")]);
  const original = vi.mocked(invoke).getMockImplementation()!;
  const target = { project_id: "project", feature_id: "feature", session_id: "Agent" };
  let finish!: (text: string) => void;
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "prompt_library_load") return Promise.resolve({
      entries: [{ key: "plain", name: "Library fixture", source: "User", body: "Library prompt", tags: [], description: null, slots: [] }],
      available_keys: ["plain"],
      targets: [{ target, label: "Agent", stopped: false }],
    });
    if (command === "prompt_library_resolve") {
      if ((args as { request: { target: unknown } }).request.target) return new Promise((resolve) => { finish = resolve; });
      return Promise.resolve("Library prompt");
    }
    return original(command, args, options);
  });
  fireEvent.click(within(draftInput().closest(".composer")!).getByRole("button", { name: "Prompt library" }));
  fireEvent.click(await screen.findByRole("button", { name: /Library fixture/ }));
  await screen.findByText("Library prompt", { selector: "pre[aria-label='Resolved prompt']" });
  await waitFor(() => expect((screen.getByRole("button", { name: "Add to draft" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Add to draft" }));
  const snapshot = client.getQueryData<WorkspaceSnapshot>(["workspace-snapshot"])!;
  act(() => client.setQueryData(["workspace-snapshot"], {
    ...snapshot, projects: snapshot.projects.map((project) => ({ ...project, features: project.features.map((feature) => ({ ...feature, sessions: [] })) })),
  }));
  await act(async () => finish("Late prompt"));
  expect(await screen.findByText("That session was removed. Choose another agent session.")).toBeTruthy();
  expect(screen.queryByRole("textbox", { name: "Draft prompt" })).toBeNull();
  expect(promptCalls()).toHaveLength(0);
  client.clear();
});

it("polls a running project check without AI work and stops when its result arrives", async () => {
  const client = await openFeature([], [], "stopped", true);
  const original = vi.mocked(invoke).getMockImplementation()!;
  const base = {
    workflow_id: "review-check", revision: 7, target: { project_id: "project", feature_id: "feature" },
    feature_name: "my-feat", branch: "my-feat", base_ref: "main", files: [], selected_path: null,
    general_feedback: "", has_prior_review: false, error: null, save_error: null, applied_suggestions: [], history: null,
    summary: { rows: [], undecided: 0, pending_suggestions: 0, failures: [] }, check_command: "cargo test",
    check: { command: "cargo test", status: "running", output: "" },
    ai: { precall: null, running: false, walkthrough_path: null, co_review_path: null, overview_running: false, overview: null,
      question_running: false, questions: [], question_error: null, comment_draft: null, ready_comment: null, harnesses: ["claude"], message: null },
  };
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "review_begin") return Promise.resolve(base);
    if (command === "review_snapshot") return Promise.resolve({ ...base, revision: 8, check: { ...base.check, status: "passed", output: "tests passed" } });
    return original(command, args, options);
  });
  vi.useFakeTimers();
  try {
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Final Review", exact: true })));
    expect(screen.getByText("Check running: cargo test")).toBeTruthy();
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    expect(screen.getByText("Check passed: cargo test")).toBeTruthy();
    expect(screen.getByLabelText("Project check output").textContent).toBe("tests passed");
    await act(async () => vi.advanceTimersByTimeAsync(5000));
    const polls = vi.mocked(invoke).mock.calls.filter(([command]) => command === "review_snapshot");
    expect(polls).toEqual([["review_snapshot", { workflowId: "review-check" }]]);
  } finally { vi.useRealTimers(); client.clear(); }
});

const completionBase = {
  workflow_id: "review-done", revision: 3, target: { project_id: "project", feature_id: "feature" },
  feature_name: "my-feat", branch: "my-feat", base_ref: "main", files: [], selected_path: null,
  general_feedback: "Rename it", has_prior_review: false, error: null, save_error: null, applied_suggestions: [], history: null,
  summary: { rows: [], undecided: 0, pending_suggestions: 0, failures: [] }, check_command: null, check: null,
  finish: { approved: 0, needs_work: 1, skipped: 0, file_comments: 0, line_comments: 0, general_feedback: true, apply_suggestions: 0,
    post_to_pr: false, submit_prompt: false, handoff: { session_id: "Agent", label: "Agent", stopped: false }, completing: false },
  ai: { precall: null, running: false, walkthrough_path: null, co_review_path: null, overview_running: false, overview: null,
    question_running: false, questions: [], question_error: null, comment_draft: null, ready_comment: null, harnesses: ["claude"], message: null },
};

it("completes once and appends the unsent feedback prompt to the agent's composer draft", async () => {
  const client = await openFeature([session("Agent", "claude")], [], "idle", true);
  fireEvent.change(draftInput(), { target: { value: "Existing message" } });
  const original = vi.mocked(invoke).getMockImplementation()!;
  let finishAct!: () => void;
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "review_begin") return Promise.resolve(completionBase);
    if (command === "review_act") return new Promise((resolve) => { finishAct = () => resolve(null); });
    if (command === "review_take_completion") return Promise.resolve({
      workflow_id: "review-done", message: "Final review: 0 approved, 1 need work — the feedback prompt is an unsent draft in Agent",
      handoff: { target: { project_id: "project", feature_id: "feature", session_id: "Agent" }, draft_prompt: "Address the feedback" },
    });
    return original(command, args, options);
  });
  fireEvent.click(screen.getByRole("button", { name: "Final Review", exact: true }));
  fireEvent.click(await screen.findByRole("button", { name: "Complete review…" }));
  const confirm = screen.getByRole("alertdialog", { name: "Complete Final Review" });
  expect(within(confirm).getByText(/opens the "address the feedback" prompt as an unsent draft in Agent/)).toBeTruthy();
  const handoff = within(confirm).getByRole("button", { name: "Complete and hand off to Agent" });
  fireEvent.click(handoff);
  fireEvent.click(handoff);
  await waitFor(() => expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "review_act")).toHaveLength(1));
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("review_act", { workflowId: "review-done", revision: 3,
    action: { kind: "complete", check_command: null, apply_suggestions: 0, handoff_session: "Agent", deliver: true } });
  await act(async () => finishAct());
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Final Review" })).toBeNull());
  expect(draftInput().value).toBe("Existing message\n\nAddress the feedback");
  expect(await screen.findByText(/unsent draft in Agent/)).toBeTruthy();
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "review_take_completion"))
    .toEqual([["review_take_completion", { workflowId: "review-done" }]]);
  expect(promptCalls()).toHaveLength(0);
  client.clear();
});

it("finishes a completion whose check ends while polling and stops polling", async () => {
  const client = await openFeature([session("Agent", "claude")], [], "idle", true);
  const original = vi.mocked(invoke).getMockImplementation()!;
  const running = { ...completionBase, check_command: "cargo test", check: { command: "cargo test", status: "running", output: "" },
    finish: { ...completionBase.finish, submit_prompt: true, completing: true } };
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "review_begin") return Promise.resolve(running);
    if (command === "review_snapshot") return Promise.resolve(null);
    if (command === "review_take_completion") return Promise.resolve({
      workflow_id: "review-done", message: "Final review: check `cargo test` passed — sent to Agent",
      handoff: { target: { project_id: "project", feature_id: "feature", session_id: "Agent" }, draft_prompt: null },
    });
    return original(command, args, options);
  });
  vi.useFakeTimers();
  try {
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Final Review", exact: true })));
    expect(screen.getByText(/Completing: the configured check is running/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "Return to review" }) as HTMLButtonElement).disabled).toBe(true);
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    expect(screen.queryByRole("dialog", { name: "Final Review" })).toBeNull();
    expect(screen.getByText(/sent to Agent/)).toBeTruthy();
    expect(draftInput().value).toBe("");
    await act(async () => vi.advanceTimersByTimeAsync(5000));
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "review_snapshot")).toHaveLength(1);
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "review_take_completion")).toHaveLength(1);
  } finally { vi.useRealTimers(); client.clear(); }
});

it("collects a completed poll result when cancellation starts while the poll is in flight", async () => {
  const client = await openFeature([session("Agent", "claude")], [], "idle", true);
  const original = vi.mocked(invoke).getMockImplementation()!;
  const running = { ...completionBase, check_command: "cargo test", check: { command: "cargo test", status: "running", output: "" },
    finish: { ...completionBase.finish, completing: true } };
  let resolvePoll!: (value: null) => void;
  let rejectAction!: (error: unknown) => void;
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "review_begin") return Promise.resolve(running);
    if (command === "review_snapshot") return new Promise((resolve) => { resolvePoll = resolve; });
    if (command === "review_act") return new Promise((_, reject) => { rejectAction = reject; });
    if (command === "review_take_completion") return Promise.resolve({
      workflow_id: "review-done", message: "Final review complete — draft in Agent",
      handoff: { target: { project_id: "project", feature_id: "feature", session_id: "Agent" }, draft_prompt: "Address the feedback" },
    });
    return original(command, args, options);
  });
  vi.useFakeTimers();
  try {
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Final Review", exact: true })));
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    fireEvent.click(screen.getByRole("button", { name: "Cancel completion" }));
    await act(async () => resolvePoll(null));
    await act(async () => rejectAction({ kind: "conflict", message: "Review is closed" }));
    expect(screen.queryByRole("dialog", { name: "Final Review" })).toBeNull();
    expect(draftInput().value).toBe("Address the feedback");
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "review_take_completion")).toHaveLength(1);
  } finally { vi.useRealTimers(); client.clear(); }
});

it("badges a feature with waiting supervised edits and opens them from the feature page", async () => {
  await openFeature([session("Agent", "claude")]);
  const initial = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (command, args, options) => {
    if (command === "supervised_edit_counts") return [{
      project_id: "project", feature_id: "feature", feature_name: "my-feat", count: 2, first_id: "e1", first_path: "src/a.ts",
    }];
    if (command === "supervised_edits_load") return { target: { project_id: "project", feature_id: "feature" }, feature_name: "my-feat", popup_hold_secs: 0, edits: [] };
    return initial(command, args, options);
  });
  const header = await screen.findByRole("button", { name: /Supervised edits\s*2/ }, { timeout: 4000 });
  expect(within(screen.getByRole("navigation", { name: "Workspace" })).getByTitle("Edits waiting for review").textContent).toBe("2");
  fireEvent.click(header);
  expect(await screen.findByRole("dialog", { name: "Supervised edits" })).toBeTruthy();
  await waitFor(() => expect(vi.mocked(invoke).mock.calls.some(([command, args]) =>
    command === "supervised_edits_load" && JSON.stringify(args) === JSON.stringify({ target: { project_id: "project", feature_id: "feature" }, context: "standard" }))).toBe(true));
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "supervised_edit_respond")).toBe(false);
});


it.each(["draft", "sending"])("guards an arrival toast target switch with %s in the current review", async (guard) => {
  const client = await openFeature([session("Agent", "claude")]);
  const original = vi.mocked(invoke).getMockImplementation()!;
  const target = { project_id: "project", feature_id: "feature" };
  const pending = {
    id: "e1", revision: "r1", kind: "diff-review", path: "src/a.ts", tool: "edit", is_new_file: false,
    agent_reason: null, diff: null, diff_error: null, old_snippet: "old", new_snippet: "new",
    requested_at: null, answered: false, unavailable: null,
    effects: { approve: "Write it", reject: "Reject it", cancel: "Cancel it", feedback_reaches_agent: true },
  };
  const first = { target, feature_name: "my-feat", popup_hold_secs: 0, edits: [pending] };
  let finish!: () => void;
  vi.mocked(invoke).mockImplementation(async (command, args, options) => {
    if (command === "supervised_edit_counts") return [];
    if (command === "supervised_edits_load") return (args as { target: typeof target }).target.feature_id === "feature"
      ? first : { target: { ...target, feature_id: "other" }, feature_name: "Docs", edits: [] };
    if (command === "supervised_edit_respond") {
      await new Promise<void>((resolve) => { finish = resolve; });
      return { message: "Approved src/a.ts", view: { ...first, edits: [{ ...pending, answered: true }] } };
    }
    return original(command, args, options);
  });
  const counts = [{ ...target, feature_name: "my-feat", count: 1, first_id: "e1", first_path: "src/a.ts" }];
  await act(async () => { client.setQueryData(["supervised-edit-counts"], counts); });
  fireEvent.click(await screen.findByRole("button", { name: /Supervised edits\s*1/ }));
  await screen.findByRole("textbox", { name: "Feedback for the agent" });
  // This toast was created before the feedback/send state changed.
  await act(async () => { client.setQueryData(["supervised-edit-counts"], [...counts, {
    ...target, feature_id: "other", feature_name: "Docs", count: 1, first_id: "e2", first_path: "README.md",
  }]); });
  const toast = (await screen.findByText("Docs: the agent wants to change README.md.")).closest(".toast") as HTMLElement;
  const review = within(toast).getByRole("button", { name: "Review", exact: true });
  if (guard === "draft") {
    fireEvent.change(screen.getByRole("textbox", { name: "Feedback for the agent" }), { target: { value: "keep feedback" } });
    fireEvent.click(review);
    expect(screen.getByText("Supervised edits · my-feat")).toBeTruthy();
    const discard = screen.getByRole("alertdialog", { name: "Discard unsent feedback" });
    fireEvent.click(within(discard).getByRole("button", { name: "Keep editing" }));
    expect((screen.getByRole("textbox", { name: "Feedback for the agent" }) as HTMLTextAreaElement).value).toBe("keep feedback");
    fireEvent.click(review);
    fireEvent.click(screen.getByRole("button", { name: "Discard and switch" }));
    expect(await screen.findByText("Supervised edits · Docs")).toBeTruthy();
  } else {
    fireEvent.click(screen.getByRole("button", { name: "Approve edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Send approval" }));
    fireEvent.click(review);
    expect(screen.getByText("Supervised edits · my-feat")).toBeTruthy();
    expect(screen.getByText(/Wait for its result before switching reviews/)).toBeTruthy();
    expect(screen.queryByText("Supervised edits · Docs")).toBeNull();
    await act(async () => finish());
    expect((await screen.findAllByText(/Approved src\/a.ts/)).length).toBeGreaterThan(0);
    fireEvent.click(review);
    expect(await screen.findByText("Supervised edits · Docs")).toBeTruthy();
  }
  client.clear();
});

it("opens prompt overrides from navigation and from a pending review AI call without leaving the review", async () => {
  const client = await openFeature([], [], "stopped", true);
  const original = vi.mocked(invoke).getMockImplementation()!;
  const overrides = (context: unknown) => ({
    context, context_label: "demo / my-feat", repo: "/demo", workdir: "/demo", harness: "claude",
    scopes: [{ scope: "global", label: "Global (all projects)", available: true, reason: null }],
    project_config_error: null,
    rows: ["review.walkthrough", "review.co_review"].map((id) => ({
      id, title: id === "review.co_review" ? "Final Review: AI co-review" : "Final Review: file walkthrough",
      summary: "", placeholders: [], source: "built_in", source_harness: null, effective_template: `${id} text`,
      default_template: `${id} text`, stored: [], revision: "r",
    })),
  });
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "prompt_overrides_load") return Promise.resolve(overrides((args as { context: unknown }).context));
    if (command === "prompt_overrides_precall_target") return Promise.resolve({
      prompt_id: "review.co_review", harness: "claude", context: { kind: "feature", project_id: "project", feature_id: "feature" },
    });
    if (command === "review_begin") return Promise.resolve({
      workflow_id: "review-id", revision: 7, target: { project_id: "project", feature_id: "feature" },
      feature_name: "my-feat", branch: "my-feat", base_ref: "main", files: [], selected_path: null,
      general_feedback: "", has_prior_review: false, error: null, save_error: null, applied_suggestions: [], history: null, summary: null,
      ai: { precall: { title: "Final Review: AI co-review", harness: "Claude", preview: "rendered", viewing: false },
        running: false, walkthrough_path: null, co_review_path: null, overview_running: false, overview: null, question_running: false,
        questions: [], question_error: null, comment_draft: null, ready_comment: null, harnesses: ["claude"], message: null },
    });
    return original(command, args, options);
  });

  fireEvent.click(screen.getByRole("button", { name: "Prompt overrides" }));
  await screen.findByRole("dialog", { name: "Prompt overrides" });
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("prompt_overrides_load",
    { context: { kind: "feature", project_id: "project", feature_id: "feature" }, harness: null }));
  fireEvent.click(screen.getByRole("button", { name: "Done" }));
  expect(screen.queryByRole("dialog", { name: "Prompt overrides" })).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: "Final Review", exact: true }));
  fireEvent.click(await screen.findByRole("button", { name: "Edit prompt" }));
  const manager = await screen.findByRole("dialog", { name: "Prompt overrides" });
  expect(within(manager).getByText(/Opened from a pending AI call/)).toBeTruthy();
  await waitFor(() => expect(within(manager).getByRole("button", { name: /Final Review: AI co-review/ }).getAttribute("aria-pressed")).toBe("true"));
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("prompt_overrides_load",
    { context: { kind: "feature", project_id: "project", feature_id: "feature" }, harness: "claude" });
  // Escape closes only the manager; the review and its pending call remain.
  fireEvent.keyDown(document.body, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Prompt overrides" })).toBeNull());
  expect(screen.getByRole("dialog", { name: "Final Review" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Continue AI call" })).toBeTruthy();
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "review_act")).toBe(false);
  client.clear();
});

it("opens dormant features from navigation and hands Open to the feature page without stopping anything", async () => {
  const snapshot: WorkspaceSnapshot = {
    projects: [{ id: "project", name: "demo", repo: "/demo", is_git: false, features: [{
      id: "feature", name: "my-feat", branch: "my-feat", workdir: "/demo", is_worktree: false,
      status: "idle", agent: "claude", mode: "vibeless", sessions: [session("Agent", "claude")],
    }] }],
    snapshot_at: "2026-10-05T00:00:00Z", stopped_session_ids: [],
  };
  vi.mocked(invoke).mockImplementation(async (command) => {
    switch (command) {
      case "get_snapshot": return snapshot;
      case "supported_harnesses": return [{ slug: "claude", display_name: "Claude" }];
      case "supported_modes": return [{ slug: "vibeless", display_name: "Vibeless", description: "" }];
      case "plan_snapshot": return { active: null, draft: null };
      case "dormancy_load": return {
        enabled: true, idle_minutes: 60, unattended_hours: 4, kill_editor_on_stop: true,
        checked_at: "2026-10-05T10:00:00Z", features: [{
          project_name: "demo", workdir: "/demo", is_worktree: false, editor_alive: false,
          idle_secs: 7200, unattended_secs: 86_400, observation: {
            target: { project_id: "project", feature_id: "feature" }, feature_name: "my-feat",
            tmux_session: "amf-my-feat", last_activity: "2026-10-05T08:00:00Z", last_accessed: "2026-10-04T10:00:00Z",
          },
        }],
      };
      default: throw new Error(`Unexpected command: ${command}`);
    }
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><App /></QueryClientProvider>);
  fireEvent.click(await screen.findByRole("button", { name: "Dormant features" }));
  const panel = within(await screen.findByRole("dialog", { name: "Dormant features" }));
  expect(await panel.findByRole("checkbox", { name: "Select my-feat" })).toBeTruthy();
  fireEvent.click(panel.getByRole("button", { name: "Open" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Dormant features" })).toBeNull());
  expect(await screen.findByRole("tab", { name: /Agent/ })).toBeTruthy();
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "dormancy_stop" || command === "stop_feature")).toBe(false);
  client.clear();
});

it("opens PR Triage from a Git feature without starting it and closes through its workflow", async () => {
  const client = await openFeature([], [], "stopped", true);
  const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "pr_triage_begin") return Promise.resolve({
      workflow_id: "triage-id", revision: 2, target: { project_id: "project", feature_id: "feature" },
      feature_name: "my-feat", branch: "my-feat", stage: "pick", loading_pr: null, review: null, precall: null,
      reply: null, write_confirm: null, harnesses: ["claude"], default_harness: "claude", error: null, notice: null,
      picker: { entries: [], include_closed: false, error: null, branch_pr: null, loading: false },
    });
    if (command === "pr_triage_act") return Promise.resolve(null);
    return original(command, args, options);
  });
  fireEvent.click(screen.getByRole("button", { name: "PR Triage", exact: true }));
  // The PR reader loads its Markdown dependencies on first use.
  await screen.findByText(/No open pull requests/, {}, { timeout: 5000 });
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("pr_triage_begin", { target: { project_id: "project", feature_id: "feature" } });
  fireEvent.click(within(screen.getByRole("dialog", { name: "PR Triage" })).getByRole("button", { name: "Close" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "PR Triage" })).toBeNull());
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("pr_triage_act", { workflowId: "triage-id", revision: 2, action: { kind: "close" } });
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "start_feature" || command === "add_session")).toBe(false);
  client.clear();
});

async function waitingPopup(client: QueryClient, entries = ["e1"]) {
  const original = vi.mocked(invoke).getMockImplementation()!;
  let waiting = entries;
  const target = { project_id: "project", feature_id: "feature" };
  const edit = (id: string) => ({
    id, revision: `rev-${id}`, kind: "diff-review", path: `${id}.ts`, tool: "edit", is_new_file: false,
    agent_reason: "Check rounding", diff: null, diff_error: null, old_snippet: "before", new_snippet: "after",
    requested_at: 1, answered: false, unavailable: null,
    effects: { approve: "Write it", reject: "Reject it", cancel: "Cancel it", feedback_reaches_agent: true },
  });
  const counts = () => waiting.length ? [{ ...target, feature_name: "my-feat", count: waiting.length,
    first_id: waiting[0], first_path: `${waiting[0]}.ts` }] : [];
  vi.mocked(invoke).mockImplementation(async (command, args, options) => {
    if (command === "supervised_edit_counts") return counts();
    if (command === "supervised_edits_load") return { target, feature_name: "my-feat", popup_hold_secs: 0, edits: waiting.map(edit) };
    if (command === "supervised_edit_respond") {
      waiting = waiting.filter((id) => id !== (args as { editId: string }).editId);
      return { message: "Approved", view: { target, feature_name: "my-feat", popup_hold_secs: 0, edits: waiting.map(edit) } };
    }
    return original(command, args, options);
  });
  await act(async () => { client.setQueryData(["supervised-edit-counts"], counts()); });
}

it("automatically opens over an agent tab, dismisses without answering and restores focus", async () => {
  const client = await openFeature([session("Agent", "claude")]);
  const terminal = screen.getByRole("button", { name: "Connect terminal" }); terminal.focus();
  await waitingPopup(client);
  const dialog = await screen.findByRole("dialog", { name: "Supervised edits" });
  expect(document.activeElement).toBe(dialog);
  expect(screen.getByRole("tab", { name: /Agent/ }).getAttribute("aria-selected")).toBe("true");
  fireEvent.keyDown(dialog, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(document.activeElement).toBe(terminal);
  await act(async () => { await client.refetchQueries({ queryKey: ["supervised-edit-counts"] }); });
  await new Promise((resolve) => setTimeout(resolve, 300));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.getByRole("button", { name: /Supervised edits\s*1/ })).toBeTruthy();
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "supervised_edit_respond")).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: /Supervised edits\s*1/ }));
  expect(await screen.findByRole("dialog", { name: "Supervised edits" })).toBeTruthy();
  client.clear();
});

it.each(["draft", "modal"])("defers the popup around a %s and opens after it is cleared", async (blocker) => {
  const client = await openFeature([session("Agent", "claude")]);
  if (blocker === "draft") fireEvent.change(draftInput(), { target: { value: "Unsent prompt" } });
  else fireEvent.click(screen.getByRole("button", { name: "New project" }));
  await waitingPopup(client);
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 300)); });
  expect(screen.queryByRole("dialog", { name: "Supervised edits" })).toBeNull();
  expect(await screen.findByText(blocker === "draft"
    ? /Automatic review waits until you save or discard your draft/
    : /Automatic review waits until you close the dialog or menu/)).toBeTruthy();
  if (blocker === "draft") {
    expect(draftInput().value).toBe("Unsent prompt");
    fireEvent.click(screen.getByRole("button", { name: "Clear", exact: true }));
  } else fireEvent.keyDown(document, { key: "Escape" });
  expect(await screen.findByRole("dialog", { name: "Supervised edits" })).toBeTruthy();
  client.clear();
});

it("advances the oldest waiting queue only after a confirmed answer", async () => {
  const client = await openFeature([session("Agent", "claude")]);
  await waitingPopup(client, ["e1", "e2"]);
  await screen.findByRole("dialog", { name: "Supervised edits" });
  expect(await screen.findByText("1 more waiting")).toBeTruthy();
  const popup = screen.getByRole("dialog", { name: "Supervised edits" });
  popup.focus();
  const loads = vi.mocked(invoke).mock.calls.filter(([command]) => command === "supervised_edits_load").length;
  expect(screen.getByRole("button", { name: /e1.ts/ }).getAttribute("aria-pressed")).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "Approve edit" }));
  fireEvent.click(screen.getByRole("button", { name: "Send approval" }));
  await waitFor(() => expect(screen.getByRole("button", { name: /e2.ts/ }).getAttribute("aria-pressed")).toBe("true"));
  await waitFor(() => expect(screen.queryByText("1 more waiting")).toBeNull());
  expect(screen.getByRole("dialog", { name: "Supervised edits" })).toBe(popup);
  expect(document.activeElement).toBe(popup);
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "supervised_edits_load")).toHaveLength(loads);
  expect(screen.getAllByRole("dialog")).toHaveLength(1);
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "supervised_edit_respond")).toHaveLength(1);
  client.clear();
});

it("lets the reviewer disable automatic opening without changing shared TUI configuration", async () => {
  const client = await openFeature([session("Agent", "claude")]);
  await waitingPopup(client);
  fireEvent.click(await screen.findByRole("checkbox", { name: "Automatically open waiting edits" }));
  fireEvent.keyDown(document, { key: "Escape" });
  expect(localStorage.getItem("amf.autoReviewEdits")).toBe("off");
  await waitingPopup(client, ["e2"]);
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 300)); });
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /Supervised edits\s*1/ }));
  expect(await screen.findByRole("dialog", { name: "Supervised edits" })).toBeTruthy();
  client.clear(); localStorage.clear();
});


it.each(["manual", "automatic disabled"])("keeps a %s review mounted through its queue and empty state", async (mode) => {
  if (mode === "automatic disabled") localStorage.setItem("amf.autoReviewEdits", "off");
  const client = await openFeature([session("Agent", "claude")]);
  // A composer draft defers automatic opening, but manual review stays available.
  fireEvent.change(draftInput(), { target: { value: "Keep this draft" } });
  await waitingPopup(client, ["e1", "e2"]);
  fireEvent.click(await screen.findByRole("button", { name: /Supervised edits\s*2/ }));
  const popup = await screen.findByRole("dialog", { name: "Supervised edits" });
  await screen.findByRole("button", { name: /e1.ts/ });
  for (const next of ["e2.ts", null]) {
    fireEvent.click(screen.getByRole("button", { name: "Approve edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Send approval" }));
    if (next) await waitFor(() => expect(screen.getByRole("button", { name: /e2.ts/ }).getAttribute("aria-pressed")).toBe("true"));
    else await screen.findByText("No edits are waiting for review");
    expect(screen.getByRole("dialog", { name: "Supervised edits" })).toBe(popup);
  }
  expect(draftInput().value).toBe("Keep this draft");
  client.clear(); localStorage.clear();
});

it("keeps an automatic review open when its auto-open preference is disabled during review", async () => {
  const client = await openFeature([session("Agent", "claude")]);
  await waitingPopup(client);
  const popup = await screen.findByRole("dialog", { name: "Supervised edits" });
  fireEvent.click(await screen.findByRole("checkbox", { name: "Automatically open waiting edits" }));
  fireEvent.click(screen.getByRole("button", { name: "Approve edit" }));
  fireEvent.click(screen.getByRole("button", { name: "Send approval" }));
  await screen.findByText("No edits are waiting for review");
  expect(screen.getByRole("dialog", { name: "Supervised edits" })).toBe(popup);
  client.clear(); localStorage.clear();
});


it("hands a PR fix to the chosen agent composer once while preserving both sessions' drafts", async () => {
  const client = await openFeature([session("Claude", "claude"), session("Codex", "codex")], [], "stopped", true);
  fireEvent.change(draftInput(), { target: { value: "Claude reminder" } });
  fireEvent.click(screen.getByRole("tab", { name: /Codex/ }));
  fireEvent.change(draftInput(), { target: { value: "Codex reminder" } });
  fireEvent.click(screen.getByRole("tab", { name: /Claude/ }));
  const original = vi.mocked(invoke).getMockImplementation()!;
  const target = { project_id: "project", feature_id: "feature", session_id: "Codex" };
  const view = {
    workflow_id: "triage-id", revision: 2, target, feature_name: "my-feat", branch: "my-feat", stage: "review",
    picker: null, loading_pr: null, precall: null, reply: null, write_confirm: null, handoff: null,
    fix_targets: [{ target, label: "Codex", harness: "codex", stopped: true }],
    fix_draft: { comment_id: 1, target, prompt: "Fix rounding" },
    harnesses: ["codex"], default_harness: "codex", error: null, notice: null,
    review: { number: 12, head_ref: "my-feat", head_sha: "abc", open_count: 1, total: 1, fetched_at: "", sort: "fetch_order", hide_resolved: false, comments: [], investigating: null },
  };
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "pr_triage_begin") return Promise.resolve(view);
    if (command === "pr_triage_act") return Promise.resolve({ ...view, handoff: { target, draft_prompt: "Fix rounding" } });
    return original(command, args, options);
  });
  fireEvent.click(screen.getByRole("button", { name: "PR Triage", exact: true }));
  fireEvent.click(await screen.findByRole("button", { name: "Open in agent composer" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "PR Triage" })).toBeNull());
  expect(draftInput().value).toBe("Codex reminder\n\nFix rounding");
  expect(document.activeElement).toBe(draftInput());
  fireEvent.click(screen.getByRole("tab", { name: /Claude/ }));
  expect(draftInput().value).toBe("Claude reminder");
  expect(promptCalls()).toHaveLength(0);
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "start_feature" || command === "start_session")).toBe(false);
  client.clear();
});

it("opens debug history from workspace navigation and preserves the session draft on close", async () => {
  const client = await openFeature([session("Agent", "codex")]);
  const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((command, args, options) => command === "debug_log_load"
    ? Promise.resolve({ entries: [], limit: 1000, shared_history: true })
    : original(command, args, options));
  fireEvent.change(draftInput(), { target: { value: "Keep this unsent" } });
  fireEvent.click(screen.getByRole("button", { name: "Debug log", exact: true }));
  const dialog = await screen.findByRole("dialog", { name: "Debug log" });
  expect(await within(dialog).findByText("No log entries yet.")).toBeTruthy();
  fireEvent.click(within(dialog).getAllByRole("button", { name: "Close", exact: true })[0]);
  expect(screen.queryByRole("dialog", { name: "Debug log" })).toBeNull();
  expect(draftInput().value).toBe("Keep this unsent");
  expect(promptCalls()).toHaveLength(0);
  client.clear();
});

it.each(["PR Triage", "PR Review"])("opens %s from a project without choosing a feature", async (label) => {
  const client = await openFeature([], [], "stopped", true);
  const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "pr_triage_begin") return Promise.resolve({
      workflow_id: "triage-project", revision: 0, target: { project_id: "project", feature_id: null },
      feature_name: "demo", stage: "pick", picker: { entries: [], loading: false, include_closed: false, error: null, branch_pr: null },
      review: null, harnesses: [], fix_targets: [],
    });
    if (command === "pr_review_begin") return Promise.resolve({
      workflow_id: "review-project", revision: 0, project_name: "demo", stage: "pick", entries: [], loading: false, files: [], summary: "", submission: null,
    });
    return original(command, args, options);
  });
  // A project with no features still offers both repository workflows.
  const snapshot = client.getQueryData<WorkspaceSnapshot>(["workspace-snapshot"])!;
  await act(async () => { client.setQueryData(["workspace-snapshot"], { ...snapshot, projects: [{ ...snapshot.projects[0], features: [] }] }); });
  fireEvent.click(screen.getByRole("button", { name: "demo", exact: true }));
  fireEvent.click(screen.getByRole("button", { name: label, exact: true }));
  expect(await screen.findByRole("dialog", { name: label })).toBeTruthy();
  expect(screen.getByRole("region", { name: "Choose a pull request" })).toBeTruthy();
  expect(vi.mocked(invoke)).toHaveBeenCalledWith(label === "PR Triage" ? "pr_triage_begin" : "pr_review_begin",
    label === "PR Triage" ? { target: { project_id: "project" } } : { projectId: "project" });
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "review_begin")).toBe(false);
  client.clear();
});

it("hides project PR actions for ordinary directories", async () => {
  const client = await openFeature([]);
  fireEvent.click(screen.getByRole("button", { name: "demo", exact: true }));
  expect(screen.queryByRole("button", { name: "PR Triage" })).toBeNull();
  expect(screen.queryByRole("button", { name: "PR Review" })).toBeNull();
  client.clear();
});
