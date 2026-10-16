// @vitest-environment jsdom
import { creationChoices } from "./creationChoices";
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import App from "../src/App";
import type { FeaturePreset, WorkspaceSnapshot } from "../src/api";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));
vi.mock("../src/TerminalPane", () => ({ default: () => <p>Offline terminal</p> }));
// Keep the App callback/routing real; the asserted native fixture exercises the actual TODO menu.
vi.mock("../src/TodoPanel", () => ({ default: ({ onLaunchNew, onPlanNew }: {
  onLaunchNew?: (id: string, title: string) => void; onPlanNew?: (id: string, title: string) => void;
}) => <><button onClick={() => onLaunchNew?.("todo", "Improve quotas")}>TODO launch entry</button>
  <button onClick={() => onPlanNew?.("todo", "Improve quotas")}>TODO plan entry</button></> }));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.resetAllMocks(); localStorage.clear(); });
it.each(["none", "full", "quick"] as const)("routes a TODO preset through App to %s with explicit fields and no prompt send", async (kind) => {
  const preset: FeaturePreset = { name: "Quota preset", branch_prefix: "feat", agent: "codex", mode: "vibe",
    review: true, enable_chrome: true, remote_control: false, plan_mode: kind !== "none", quick_plan: kind === "quick" };
  const snapshot: WorkspaceSnapshot = { projects: [{ id: "project", name: "demo", repo: "/tmp/demo", is_git: true, features: [] }],
    snapshot_at: "2026-10-14T00:00:00Z", stopped_session_ids: [] };
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "get_snapshot") return snapshot;
    if (command === "project_harness_choices") return creationChoices(["codex"]);
    if (command === "supported_harnesses" || command === "allowed_harnesses") return [{ slug: "codex", display_name: "Codex" }];
    if (command === "supported_modes") return [{ slug: "vibe", display_name: "Vibe", description: "" }];
    if (command === "feature_presets_choices") return [preset];
    if (command === "worktree_hook_prompt") return null;
    if (command === "plan_snapshot" || command === "plan_begin_todo_new") return { active: null, precall: null };
    if (command === "todo_launch_new_feature") {
      snapshot.projects[0].features.push({ id: "created", name: "feat/improve-quotas", branch: "feat/improve-quotas", workdir: "/tmp/demo/.worktrees/quotas",
        is_worktree: true, status: "idle", agent: "codex", mode: "vibe", sessions: [{ id: "agent", kind: "codex", label: "Codex", tmux_window: "codex" }] });
      return { target: { project_id: "project", feature_id: "created", session_id: "agent" }, draft_prompt: "Implement quota TODO", reused_session: false };
    }
    return null;
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } }); clients.push(client);
  render(<QueryClientProvider client={client}><App /></QueryClientProvider>);
  fireEvent.click(await screen.findByRole("button", { name: "Global TODOs" }));
  fireEvent.click(await screen.findByRole("button", { name: "TODO launch entry" }));
  await screen.findByRole("option", { name: "Quota preset" });
  fireEvent.change(screen.getByLabelText("Feature preset"), { target: { value: "0" } });
  const button = screen.getByRole("button", { name: kind === "none" ? "Create and start" : kind === "quick" ? "Start Quick Plan" : "Start plan" });
  await waitFor(() => expect(button).toHaveProperty("disabled", false)); fireEvent.click(button);
  const expectedRequest = expect.objectContaining({ project_name: "demo", branch: "feat/improve-quotas", agent: "codex", mode: "vibe", review: true,
    enable_chrome: true, plan_mode: kind !== "none", use_worktree: true });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith(kind === "none" ? "todo_launch_new_feature" : "plan_begin_todo_new",
    kind === "none" ? { todoId: "todo", request: expectedRequest, approved: false } : { todoId: "todo", request: expectedRequest, quick: kind === "quick" }));
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === (kind === "none" ? "plan_begin_todo_new" : "todo_launch_new_feature"))).toHaveLength(0);
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "terminal_submit_prompt" || command === "terminal_input")).toHaveLength(0);
  if (kind === "none") expect(await screen.findByRole("textbox", { name: "Draft prompt" })).toHaveProperty("value", "Implement quota TODO");
});
