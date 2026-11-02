// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import TodoPanel from "../src/TodoPanel";
import { QuickTodoDialog } from "../src/TodoNextDialogs";
import type { NextTodoCandidate, NextTodoPreview, Todo } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });

const todo = (over: Partial<Todo> = {}): Todo => ({
  id: "t1", list_id: "l1", title: "Fix flaky test", body: "see CI", priority: "high", sort_order: 0,
  work: { status: "not_started", agent_session_id: null }, linked_feature_id: null, created_at: "", updated_at: "", ...over,
});
const candidate = (over: Partial<NextTodoCandidate> = {}): NextTodoCandidate => ({
  todo: todo(), scope_kind: "project", scope_label: "Project · demo", host: null, ...over,
});
const scope = { kind: "project", project_id: "p" } as const;
const target = { project_id: "p", feature_id: "f" };

function mount(preview: (args: { request: { skipped_ids: string[]; include_global: boolean } }) => NextTodoPreview, extra: Record<string, unknown> = {}) {
  const onLaunch = vi.fn(async () => {});
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "todo_list") return { list: { id: "l1", scope, feature_id: null, carry_over: null, created_at: "", updated_at: "" }, todos: [todo()] };
    if (command === "todo_next_preview") return preview(args as never);
    return null;
  });
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <TodoPanel scope={scope} title="Project TODOs" onError={vi.fn()} onLaunch={onLaunch}
      launchTargets={[{ label: "feature-f", target }]} {...extra} /></QueryClientProvider>);
  return onLaunch;
}

const launches = () => vi.mocked(invoke).mock.calls.filter(([c]) => c === "todo_launch_agent" || c === "todo_launch_new_feature");

it("previews the pick and starts nothing until the ordinary Start dialog is confirmed", async () => {
  const onLaunch = mount(() => ({ outcome: "ready", candidate: candidate() }));
  fireEvent.click(await screen.findByRole("button", { name: /Implement next/ }));
  const card = await screen.findByLabelText("Next TODO");
  expect(card.textContent).toContain("Fix flaky test");
  expect(card.textContent).toContain("Project · demo");
  expect(card.textContent).toContain("High priority");
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("todo_next_preview", { request: {
    project_id: "p", feature_id: null, include_project: true, include_global: true, skipped_ids: [] } });
  expect(onLaunch).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: /^Start…/ }));
  const start = await screen.findByRole("dialog", { name: "Start TODO" });
  expect(start.textContent).toContain("Fix flaky test");
  expect(onLaunch).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Start agent" }));
  await waitFor(() => expect(onLaunch).toHaveBeenCalledWith("t1", target));
  expect(launches()).toHaveLength(0); // the real launch command is App's, mocked here
});

it("offers already-started work as a question and skip re-previews with accumulated ids", async () => {
  const preview = vi.fn((args: { request: { skipped_ids: string[] } }): NextTodoPreview =>
    args.request.skipped_ids.length === 0
      ? { outcome: "started", reason: "It already has a session.", candidate: candidate({ todo: todo({ id: "t9", title: "Held" }) }) }
      : { outcome: "empty", message: "No other TODOs left to implement" });
  mount(preview, { onJump: vi.fn() });
  fireEvent.click(await screen.findByRole("button", { name: /Implement next/ }));
  expect((await screen.findByRole("note")).textContent).toContain("It already has a session.");
  expect(screen.getByRole("button", { name: "Go to work" })).toBeTruthy();
  expect(screen.getByRole("button", { name: /Start another agent/ })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Skip to next" }));
  expect(await screen.findByText("No other TODOs left to implement")).toBeTruthy();
  expect(preview.mock.calls[1][0].request.skipped_ids).toEqual(["t9"]);
});

it("re-previews when a list is switched off", async () => {
  const preview = vi.fn((): NextTodoPreview => ({ outcome: "empty", message: "No TODOs left to implement" }));
  mount(preview);
  fireEvent.click(await screen.findByRole("button", { name: /Implement next/ }));
  await screen.findByText("No TODOs left to implement");
  fireEvent.click(screen.getByRole("checkbox", { name: "Global list" }));
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("todo_next_preview", expect.objectContaining({
    request: expect.objectContaining({ include_global: false }) })));
});

it("quick capture names the list, trims, and reports where it went", async () => {
  const onSaved = vi.fn(); const onClose = vi.fn();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "todo_quick_capture_target") return "Worktree · billing-retry";
    if (command === "todo_quick_capture") return { todo: todo(), scope_label: "Worktree · billing-retry" };
    return null;
  });
  render(<QueryClientProvider client={new QueryClient()}>
    <QuickTodoDialog target={target} onSaved={onSaved} onError={vi.fn()} onClose={onClose} /></QueryClientProvider>);
  expect(await screen.findByText("Adds to Worktree · billing-retry")).toBeTruthy();
  const add = screen.getByRole("button", { name: "Add TODO" });
  expect((add as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(screen.getByLabelText("Quick TODO title"), { target: { value: "  remember this  " } });
  fireEvent.click(add);
  await waitFor(() => expect(onSaved).toHaveBeenCalledWith("Worktree · billing-retry"));
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("todo_quick_capture", { target, title: "  remember this  " });
  // One click is one capture, not a click handler plus a form submit.
  expect(vi.mocked(invoke).mock.calls.filter(([c]) => c === "todo_quick_capture")).toHaveLength(1);
  expect(onClose).toHaveBeenCalled();
});

it("quick capture keeps the text and reports a refusal", async () => {
  const onError = vi.fn(); const onClose = vi.fn();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "todo_quick_capture_target") return "Project · demo";
    if (command === "todo_quick_capture") throw { kind: "not_found", message: "Feature was deleted" };
    return null;
  });
  render(<QueryClientProvider client={new QueryClient()}>
    <QuickTodoDialog target={target} onSaved={vi.fn()} onError={onError} onClose={onClose} /></QueryClientProvider>);
  await screen.findByText("Adds to Project · demo");
  fireEvent.change(screen.getByLabelText("Quick TODO title"), { target: { value: "note" } });
  fireEvent.click(screen.getByRole("button", { name: "Add TODO" }));
  await waitFor(() => expect(onError).toHaveBeenCalledWith(expect.objectContaining({ message: "Feature was deleted" })));
  expect(onClose).not.toHaveBeenCalled();
  expect((screen.getByLabelText("Quick TODO title") as HTMLInputElement).value).toBe("note");
});
