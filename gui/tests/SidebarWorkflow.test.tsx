// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import App from "../src/App";
import type { WorkspaceSnapshot } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));
vi.mock("../src/TerminalPane", () => ({ default: () => null }));
vi.mock("../src/TodoPanel", () => ({ default: () => <p>TODO panel</p> }));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function snapshot(projectCollapsed: boolean, featureCollapsed: boolean): WorkspaceSnapshot {
  return {
    projects: [{
      id: "project", name: "demo", repo: "/home/me/demo", is_git: true, collapsed: projectCollapsed,
      features: [{
        id: "feature", name: "my-feat", branch: "my-feat", workdir: "/home/me/demo/.worktrees/my-feat",
        is_worktree: true, status: "idle", agent: "claude", mode: "vibe", collapsed: featureCollapsed,
        sessions: [
          { id: "claude", kind: "claude", label: "Claude 1", tmux_window: "claude" },
          { id: "codex", kind: "codex", label: "Codex 1", tmux_window: "codex" },
          { id: "todos", kind: "todos", label: "TODOs", tmux_window: "" },
        ],
      }],
    }],
    snapshot_at: "2026-10-06T00:00:00Z",
    stopped_session_ids: [],
    sidebar: {
      projects: { project: { repo_display: "~/demo" } },
      features: { feature: {
        workdir_display: "~/demo/.worktrees/my-feat", created_age: "3h ago", issue: null, summary_age: null,
        usage: null, pr: { state: "merged", number: 12 }, thinking: false, waiting_for_input: false, pending_input: false,
      } },
      sessions: {
        claude: { status_text: "usage 1.2k eff", context: { text: "Ctx 42%", band: "normal", stale: false, pending_reset: false }, icon: null, icon_nerd: null },
      },
    },
  };
}

function mount(handleCollapse: (args: Record<string, unknown>) => Promise<WorkspaceSnapshot>) {
  let current = snapshot(false, true);
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "get_snapshot": return current;
      case "set_collapsed": {
        const next = await handleCollapse(args as Record<string, unknown>);
        current = next;
        return next;
      }
      case "supported_harnesses": return [{ slug: "claude", display_name: "Claude" }];
      case "supported_modes": return [{ slug: "vibe", display_name: "Vibe", description: "" }];
      case "plan_snapshot": return { active: null, precall: null };
      default: throw new Error(`Unexpected command: ${command}`);
    }
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><App /></QueryClientProvider>);
  return { client, setCurrent: (next: WorkspaceSnapshot) => { current = next; } };
}

const tree = () => within(screen.getByRole("navigation", { name: "Workspace" }));
const collapseCalls = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "set_collapsed");

it("persists collapse through the shared store and adopts a TUI-side change", async () => {
  let finishProjectWrite!: () => void;
  const projectWrite = new Promise<void>((resolve) => { finishProjectWrite = resolve; });
  const { client, setCurrent } = mount(async (args) => {
    const target = args.target as { feature_id: string | null };
    if (target.feature_id !== null) return snapshot(false, args.collapsed as boolean);
    await projectWrite;
    return snapshot(args.collapsed as boolean, false);
  });
  expect(await screen.findByText("PR #12 merged")).toBeTruthy();
  expect(screen.queryByText("Claude 1")).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: "Show sessions of my-feat" }));
  expect(await screen.findByText("Claude 1")).toBeTruthy();
  expect(collapseCalls()).toEqual([["set_collapsed", { target: { project_id: "project", feature_id: "feature" }, collapsed: false }]]);
  expect(screen.getByText("~/demo/.worktrees/my-feat")).toBeTruthy();
  expect(screen.getByText("Ctx 42%")).toBeTruthy();
  expect(screen.getByText("usage 1.2k eff")).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "Collapse demo" }));
  // Shown before the write comes back.
  await waitFor(() => expect(tree().queryByRole("button", { name: "my-feat", exact: true })).toBeNull());
  expect(collapseCalls()[1]).toEqual(["set_collapsed", { target: { project_id: "project", feature_id: null }, collapsed: true }]);
  await act(async () => { finishProjectWrite(); });
  expect(tree().queryByRole("button", { name: "my-feat", exact: true })).toBeNull();

  // The TUI expands the project in the shared store; the next poll shows it.
  setCurrent(snapshot(false, false));
  await act(async () => { await client.invalidateQueries({ queryKey: ["workspace-snapshot"] }); });
  expect(await tree().findByRole("button", { name: "my-feat", exact: true })).toBeTruthy();
  expect(tree().getByText("Codex 1")).toBeTruthy();
  client.clear();
});

it("restores the previous state and reports a refused collapse write", async () => {
  const { client } = mount(async () => { throw { kind: "not_found", message: "That project or feature was deleted; refresh and retry" }; });
  fireEvent.click(await screen.findByRole("button", { name: "Collapse demo" }));
  expect(await screen.findByText("That project or feature was deleted; refresh and retry")).toBeTruthy();
  expect(tree().getByRole("button", { name: "my-feat", exact: true })).toBeTruthy();
  client.clear();
});

it("opens a session's tab, or the TODO list, from its tree row", async () => {
  const { client } = mount(async () => snapshot(false, false));
  fireEvent.click(await screen.findByRole("button", { name: "Show sessions of my-feat" }));
  fireEvent.click(await tree().findByRole("button", { name: /Codex 1/ }));
  await waitFor(() => expect(screen.getByRole("tab", { name: /Codex 1/ }).getAttribute("aria-selected")).toBe("true"));
  expect(tree().getByRole("button", { name: /Codex 1/ }).getAttribute("aria-current")).toBe("page");
  fireEvent.click(tree().getByRole("button", { name: "TODOs", exact: true }));
  await waitFor(() => expect(screen.getByRole("tab", { name: "TODOs" }).getAttribute("aria-selected")).toBe("true"));
  expect(screen.getByText("TODO panel")).toBeTruthy();
  client.clear();
});
