// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TodoNewFeatureForm } from "../src/App";

vi.mock("../src/TerminalPane", () => ({ default: () => null }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));

afterEach(cleanup);

const project = {
  id: "project-1",
  name: "demo",
  repo: "/tmp/demo",
  is_git: true,
  features: [],
};

describe("TodoNewFeatureForm", () => {
  it("submits a direct TODO launch as a worktree feature without planning", async () => {
    const onSubmit = vi.fn();
    render(
      <QueryClientProvider client={new QueryClient()}><TodoNewFeatureForm
        kind="launch"
        todoTitle="Improve the API"
        projects={[project] as Parameters<typeof TodoNewFeatureForm>[0]["projects"]}
        agents={[{ slug: "claude", display_name: "Claude" }]}
        modes={[{ slug: "vibe", display_name: "Vibe" }]}
        pending={false}
        onCancel={vi.fn()}
        onSubmit={onSubmit}
      /></QueryClientProvider>,
    );

    const button = screen.getByRole("button", { name: "Create and start" });
    await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(button);
    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({
      project_name: "demo",
      branch: "improve-the-api",
      plan_mode: false,
      use_worktree: true,
      dry_run: false,
    }));
  });
});
