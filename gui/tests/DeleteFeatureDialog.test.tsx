// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import DeleteFeatureDialog from "../src/DeleteFeatureDialog";

afterEach(cleanup);

it("confirms without a TODO choice until the backend asks for one", () => {
  const onConfirm = vi.fn();
  render(<DeleteFeatureDialog projectName="demo" featureName="my-feat" isWorktree
    unfinished={null} busy={false} onConfirm={onConfirm} onClose={vi.fn()} />);

  expect(screen.getByText(/removes the worktree/)).toBeTruthy();
  expect(screen.queryByRole("radio")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Delete feature" }));
  expect(onConfirm).toHaveBeenCalledWith(null);
});

it("sends the chosen disposition for unfinished TODOs", () => {
  const onConfirm = vi.fn();
  render(<DeleteFeatureDialog projectName="demo" featureName="my-feat" isWorktree
    unfinished={2} busy={false} onConfirm={onConfirm} onClose={vi.fn()} />);

  expect(screen.getByText("Its worktree has 2 unfinished TODOs.")).toBeTruthy();
  fireEvent.click(screen.getByRole("radio", { name: "Move them to the global list" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete feature" }));
  expect(onConfirm).toHaveBeenCalledWith("move_to_global");
});
