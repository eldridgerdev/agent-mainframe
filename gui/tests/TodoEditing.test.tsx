// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import TodoPanel from "../src/TodoPanel";
import type { Todo, TodoListView } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });

const todo = (over: Partial<Todo> = {}): Todo => ({
  id: "t1", list_id: "l1", title: "Write docs", body: "cover the API", priority: "med", sort_order: 0,
  work: { status: "not_started", agent_session_id: null }, linked_feature_id: null,
  created_at: "", updated_at: "", ...over,
});
const scope = { kind: "global" } as const;
const view = (todos: Todo[], carry: string | null = null): TodoListView => ({
  list: { id: "l1", scope, feature_id: null, carry_over: carry, created_at: "", updated_at: "" }, todos,
});

function mount(todos: Todo[], handlers: Record<string, (args: unknown) => unknown>, extra: Record<string, unknown> = {}, carry: string | null = null) {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "todo_list") return view(todos, carry);
    return handlers[command as string]?.(args) ?? null;
  });
  const onError = vi.fn();
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <TodoPanel scope={scope} title="TODOs" onError={onError} {...extra} /></QueryClientProvider>);
  return onError;
}

async function openEdit() {
  fireEvent.click(await screen.findByRole("button", { name: "Actions for Write docs" }));
  fireEvent.click(await screen.findByRole("menuitem", { name: /Edit/ }));
}

it("saves title, notes and priority bound to the values the editor opened with", async () => {
  const update = vi.fn(() => ({ outcome: "saved", todo: todo({ title: "Write better docs" }) }));
  mount([todo()], { todo_update: update });
  await openEdit();
  fireEvent.change(screen.getByLabelText("TODO title"), { target: { value: "Write better docs" } });
  fireEvent.change(screen.getByLabelText("TODO notes"), { target: { value: "more detail" } });
  fireEvent.click(screen.getAllByText("High")[0]);
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
  expect(update).toHaveBeenCalledWith({ request: {
    list_id: "l1", todo_id: "t1",
    base: { title: "Write docs", body: "cover the API", priority: "med" },
    fields: { title: "Write better docs", body: "more detail", priority: "high" },
  } });
});

it("shows the stored value on a stale save and never overwrites without a second explicit action", async () => {
  const update = vi.fn()
    .mockReturnValueOnce({ outcome: "conflict", current: todo({ title: "Changed in TUI", priority: "high" }) })
    .mockReturnValueOnce({ outcome: "saved", todo: todo({ title: "Mine" }) });
  mount([todo()], { todo_update: update });
  await openEdit();
  fireEvent.change(screen.getByLabelText("TODO title"), { target: { value: "Mine" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(await screen.findByText("Changed in TUI")).toBeTruthy();
  expect(update).toHaveBeenCalledTimes(1);
  // The dialog keeps my text and offers the explicit overwrite, rebased on what is stored.
  expect((screen.getByLabelText("TODO title") as HTMLInputElement).value).toBe("Mine");
  fireEvent.click(screen.getByRole("button", { name: "Overwrite with my edits" }));
  await waitFor(() => expect(update).toHaveBeenCalledTimes(2));
  const second = update.mock.calls[1][0] as { request: { base: { title: string } } };
  expect(second.request.base.title).toBe("Changed in TUI");
});

it("can discard my edits in favour of the stored value after a conflict", async () => {
  const update = vi.fn(() => ({ outcome: "conflict", current: todo({ title: "Changed in TUI", body: null }) }));
  mount([todo()], { todo_update: update });
  await openEdit();
  fireEvent.change(screen.getByLabelText("TODO title"), { target: { value: "Mine" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  fireEvent.click(await screen.findByRole("button", { name: "Discard my edits and use this" }));
  expect((screen.getByLabelText("TODO title") as HTMLInputElement).value).toBe("Changed in TUI");
  expect(update).toHaveBeenCalledTimes(1);
});

it("edits the scratchpad bound to the list id and the text it opened with", async () => {
  const save = vi.fn(() => ({ outcome: "saved", list_id: "l1", carry_over: "next: tests" }));
  mount([], { todo_update_scratchpad: save }, {}, "left off here");
  expect(await screen.findByText("left off here")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  fireEvent.change(screen.getByLabelText("Scratchpad text"), { target: { value: "next: tests" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(save).toHaveBeenCalledWith({ request: {
    scope, host_feature_id: null, list_id: "l1", base: "left off here", text: "next: tests" } }));
});

it("jumps to the linked work, and says why when there is none", async () => {
  const onJump = vi.fn();
  const target = { project_id: "p", feature_id: "f", session_id: "s" };
  const jumpTarget = vi.fn()
    .mockReturnValueOnce({ kind: "session", target })
    .mockReturnValueOnce({ kind: "none", message: "The feature planned for this TODO no longer exists; the link was cleared" });
  const onError = mount([todo({ work: { status: "in_progress", agent_session_id: "s" } })],
    { todo_work_target: jumpTarget }, { onJump });
  const button = await screen.findByRole("button", { name: "Go to the work for Write docs" });
  fireEvent.click(button);
  await waitFor(() => expect(onJump).toHaveBeenCalledWith({ kind: "session", target }));
  fireEvent.click(button);
  await waitFor(() => expect(onError).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringContaining("link was cleared") })));
  expect(onJump).toHaveBeenCalledTimes(1);
});

it("offers no jump for a TODO with no recorded work", async () => {
  mount([todo()], {}, { onJump: vi.fn() });
  await screen.findByText("Write docs");
  expect(screen.queryByRole("button", { name: /Go to the work/ })).toBeNull();
});
