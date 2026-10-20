// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import App from "../src/App";
import type { CollapseTarget, Feature, WorkspaceSnapshot } from "../src/api";
import { resetSidebarPrefsForTest } from "../src/sidebarPrefs";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));
vi.mock("../src/TerminalPane", () => ({ default: () => null }));
vi.mock("../src/TodoPanel", () => ({ default: () => null }));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  localStorage.clear();
  resetSidebarPrefsForTest();
});

const feature = (id: string): Feature => ({
  id, name: id, branch: id, workdir: `/${id}`, is_worktree: true, status: "stopped",
  agent: "claude", mode: "vibeless", sessions: [],
});

/** Invoice (stored collapsed) holds fix-auth; Docs (expanded) holds landing.
 *  `set_collapsed` waits for `release` so a test can hold a write in flight. */
async function renderWorkspace() {
  let snapshot: WorkspaceSnapshot = {
    projects: [
      { id: "invoice", name: "Invoice", repo: "/invoice", is_git: true, collapsed: true, features: [feature("fix-auth")] },
      { id: "docs", name: "Docs", repo: "/docs", is_git: true, collapsed: false, features: [feature("landing")] },
    ],
    snapshot_at: "2026-10-19T00:00:00Z",
    stopped_session_ids: [],
  };
  const writes: CollapseTarget[] = [];
  const pending: (() => void)[] = [];
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "get_snapshot": return snapshot;
      case "supported_harnesses": return [{ slug: "claude", display_name: "Claude" }];
      case "supported_modes": return [{ slug: "vibeless", display_name: "Vibeless", description: "" }];
      case "plan_snapshot": return { active: null, draft: null };
      case "set_collapsed": {
        const { target, collapsed } = args as { target: CollapseTarget; collapsed: boolean };
        writes.push(target);
        await new Promise<void>((resolve) => pending.push(resolve));
        snapshot = { ...snapshot, projects: snapshot.projects.map((project) =>
          project.id === target.project_id ? { ...project, collapsed } : project) };
        return snapshot;
      }
      default: return null;
    }
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><App /></QueryClientProvider>);
  const filter = await screen.findByRole("searchbox", { name: "Filter projects and features" });
  return { filter, writes, release: () => pending.splice(0).forEach((resolve) => resolve()) };
}

const typeFilter = (input: HTMLElement, value: string) => fireEvent.change(input, { target: { value } });
const activeProject = () => document.querySelector(".tree-project-head-active")?.textContent ?? null;

it("claims / only when the filter can take focus", async () => {
  const { filter } = await renderWorkspace();
  expect(fireEvent.keyDown(document.body, { key: "/" })).toBe(false);
  expect(document.activeElement).toBe(filter);
  filter.blur();
  fireEvent.click(screen.getByRole("button", { name: "Hide projects sidebar" }));
  // The hidden sidebar keeps the input mounted; the key must pass through.
  expect(fireEvent.keyDown(document.body, { key: "/" })).toBe(true);
  expect(document.activeElement).not.toBe(filter);
});

it("does nothing on Enter with an empty filter", async () => {
  const { filter } = await renderWorkspace();
  fireEvent.click(screen.getByText("/docs"));
  expect(activeProject()).toContain("Docs");
  typeFilter(filter, "   ");
  fireEvent.keyDown(filter, { key: "Enter" });
  expect(activeProject()).toContain("Docs");
});

it("collapses filter-opened projects as view state and opens only visible matches on Enter", async () => {
  const { filter, writes } = await renderWorkspace();
  typeFilter(filter, "auth");
  expect(screen.getByRole("button", { name: "fix-auth" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Collapse all projects" }));
  // Invoice's stored flag already says collapsed: nothing is written.
  await waitFor(() => expect(screen.queryByRole("button", { name: "fix-auth" })).toBeNull());
  expect(screen.getByRole("button", { name: "Expand all projects" })).toBeTruthy();
  expect(writes).toEqual([]);
  // fix-auth is no longer on screen, so Enter opens its project.
  fireEvent.keyDown(filter, { key: "Enter" });
  expect(activeProject()).toContain("Invoice");
  expect(document.querySelector(".tree-feature-active")).toBeNull();
  // A new filter starts with every match open again.
  typeFilter(filter, "fix");
  expect(screen.getByRole("button", { name: "fix-auth" })).toBeTruthy();
});

it("runs one collapse-all at a time", async () => {
  const { writes, release } = await renderWorkspace();
  const button = screen.getByRole("button", { name: "Collapse all projects" });
  fireEvent.click(button);
  fireEvent.click(button);
  await waitFor(() => expect(writes).toHaveLength(1));
  expect((button as HTMLButtonElement).disabled).toBe(true);
  await act(async () => { release(); });
  await waitFor(() => expect(screen.getByRole("button", { name: "Expand all projects" })).toBeTruthy());
  // Only Docs needed a write, and the second click started no second loop.
  expect(writes).toEqual([{ project_id: "docs", feature_id: null }]);
  expect((screen.getByRole("button", { name: "Expand all projects" }) as HTMLButtonElement).disabled).toBe(false);
});
