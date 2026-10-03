// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import DeleteFeatureDialog from "../src/DeleteFeatureDialog";

afterEach(cleanup);

const todoHost = {
  list_id: "project-list", todo_count: 3,
  candidates: [{ feature_id: "first", name: "First feature" }, { feature_id: "second", name: "Second feature" }],
};

it("confirms without a TODO choice until the backend asks for one", () => {
  const onConfirm = vi.fn();
  render(<DeleteFeatureDialog projectName="demo" featureName="my-feat" isWorktree
    unfinished={null} busy={false} onConfirm={onConfirm} onClose={vi.fn()} />);

  expect(screen.getByText(/removes the worktree/)).toBeTruthy();
  expect(screen.queryByRole("radio")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Delete feature" }));
  expect(onConfirm).toHaveBeenCalledWith(null, null);
});

it("sends the chosen disposition for unfinished TODOs", () => {
  const onConfirm = vi.fn();
  render(<DeleteFeatureDialog projectName="demo" featureName="my-feat" isWorktree
    unfinished={2} busy={false} onConfirm={onConfirm} onClose={vi.fn()} />);

  expect(screen.getByText("Its worktree has 2 unfinished TODOs.")).toBeTruthy();
  fireEvent.click(screen.getByRole("radio", { name: "Move them to the global list" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete feature" }));
  expect(onConfirm).toHaveBeenCalledWith("move_to_global", null);
});

it("keeps the project list on the first survivor by default", () => {
  const onConfirm = vi.fn();
  render(<DeleteFeatureDialog projectName="demo" featureName="my-feat" isWorktree={false}
    unfinished={null} todoHost={todoHost} busy={false} onConfirm={onConfirm} onClose={vi.fn()} />);

  expect((screen.getByRole("radio", { name: "Keep on First feature" }) as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Delete feature" }));
  expect(onConfirm).toHaveBeenCalledWith(null, { list_id: "project-list", feature_id: "first" });
});

it.each([
  { label: "Keep on Second feature", featureId: "second" },
  { label: "Delete the project list and all of its TODOs", featureId: null },
])("submits the explicit project-list choice: $label", ({ label, featureId }) => {
  const onConfirm = vi.fn();
  render(<DeleteFeatureDialog projectName="demo" featureName="my-feat" isWorktree
    unfinished={2} todoHost={todoHost} busy={false} onConfirm={onConfirm} onClose={vi.fn()} />);

  fireEvent.click(screen.getByRole("radio", { name: "Move them to the global list" }));
  fireEvent.click(screen.getByRole("radio", { name: label }));
  fireEvent.click(screen.getByRole("button", { name: "Delete feature" }));
  expect(onConfirm).toHaveBeenCalledWith("move_to_global", { list_id: "project-list", feature_id: featureId });
});

it("cancels the host choice without confirming deletion", () => {
  const onConfirm = vi.fn();
  const onClose = vi.fn();
  render(<DeleteFeatureDialog projectName="demo" featureName="my-feat" isWorktree={false}
    unfinished={null} todoHost={todoHost} busy={false} onConfirm={onConfirm} onClose={onClose} />);

  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(onClose).toHaveBeenCalledOnce();
  expect(onConfirm).not.toHaveBeenCalled();
});
