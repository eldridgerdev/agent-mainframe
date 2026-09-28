// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { TodoNewFeatureForm } from "../src/App";

vi.mock("../src/TerminalPane", () => ({ default: () => null }));

afterEach(cleanup);

const project = {
  id: "project-1",
  name: "demo",
  repo: "/tmp/demo",
  is_git: true,
  features: [],
};

describe("TodoNewFeatureForm", () => {
  it("submits a direct TODO launch as a worktree feature without planning", () => {
    const onSubmit = vi.fn();
    render(
      <TodoNewFeatureForm
        kind="launch"
        todoTitle="Improve the API"
        projects={[project] as Parameters<typeof TodoNewFeatureForm>[0]["projects"]}
        agents={[{ slug: "claude", display_name: "Claude" }]}
        modes={[{ slug: "vibe", display_name: "Vibe" }]}
        pending={false}
        onCancel={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Create and start" }));
    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({
      project_name: "demo",
      branch: "improve-the-api",
      plan_mode: false,
      use_worktree: true,
      dry_run: false,
    }));
  });
});
