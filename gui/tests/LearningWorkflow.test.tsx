// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import App from "../src/App";
import type { LearningView, WorkspaceSnapshot } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));
vi.mock("../src/TerminalPane", () => ({ default: () => null }));
vi.mock("../src/TodoPanel", () => ({ default: () => null }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

it("opens Learning from a feature and gates the editing handoff before showing an unsent terminal draft", async () => {
  const snapshot: WorkspaceSnapshot = {
    projects: [{ id: "project", name: "demo", repo: "/demo", is_git: false,
      features: [{ id: "feature", name: "my-feat", branch: "my-feat", workdir: "/demo", is_worktree: false,
        status: "idle", agent: "claude", mode: "vibeless", sessions: [{ id: "shell", label: "Shell", kind: "terminal", tmux_window: "shell" }] }] }],
    snapshot_at: "2026-10-01T00:00:00Z", stopped_session_ids: [],
  };
  const learning: LearningView = {
    workflow_id: "workflow", revision: 3, target: { project_id: "project", feature_id: "feature" }, feature_name: "my-feat",
    scope: "repo_tree", is_git: false, entries: [], content_path: null, content: [], content_line_labels: [], content_error: null,
    anchor: "this whole project", harness: "claude", harnesses: ["claude"], level: "newcomer", history_saved: true,
    error: null, notice: null, qa: [{ id: "answer", parent_id: null, question: "Explain the project", answer: "An answer", anchor: "this whole project",
      status: "answered", intent: "explain", harness: "claude", run_mode: "this file only", error: null, drift: null, spawned_session_id: null }],
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
        snapshot.projects[0].features[0].sessions.push({ id: "agent", label: "Learning agent", kind: "claude", tmux_window: "claude" });
        return { target: { project_id: "project", feature_id: "feature", session_id: "agent" }, draft_prompt: "Editable learning seed" };
      }
      default: throw new Error(`Unexpected command: ${command}`);
    }
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><App /></QueryClientProvider>);
  fireEvent.click(await screen.findByRole("button", { name: "my-feat", exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "Learning", exact: true }));
  await screen.findByRole("dialog", { name: "Learning", exact: true });
  fireEvent.click(screen.getByRole("button", { name: "Open editing agent" }));
  const approval = await screen.findByRole("dialog", { name: "Approve Learning agent" });
  expect(screen.queryByRole("textbox", { name: "Draft prompt" })).toBeNull();
  fireEvent.click(within(approval).getByRole("button", { name: "Cancel" }));
  expect(screen.getByRole("dialog", { name: "Learning", exact: true })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Open editing agent" }));
  const retry = await screen.findByRole("dialog", { name: "Approve Learning agent" });
  fireEvent.click(within(retry).getByRole("button", { name: "Start anyway" }));
  const draft = await screen.findByRole("textbox", { name: "Draft prompt" }) as HTMLTextAreaElement;
  expect(draft.value).toBe("Editable learning seed");
  expect(screen.queryByRole("dialog", { name: "Learning", exact: true })).toBeNull();
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "terminal_submit_prompt")).toBe(false);
  client.clear();
});
