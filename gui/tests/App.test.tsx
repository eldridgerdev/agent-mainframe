// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import App from "../src/App";
import type { Feature, FeatureSession, WorkspaceSnapshot } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));
vi.mock("../src/TerminalPane", () => ({ default: () => null }));
vi.mock("../src/TodoPanel", () => ({ default: () => null }));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function session(id: string, kind = "terminal"): FeatureSession {
  return { id, kind, label: id, tmux_window: id };
}

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
  client.clear();
});
