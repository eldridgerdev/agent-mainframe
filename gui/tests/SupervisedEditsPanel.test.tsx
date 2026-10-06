// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import SupervisedEditsPanel, { usePendingEdits } from "../src/SupervisedEditsPanel";
import type { PendingEditCount, SupervisedEdit, SupervisedEditsView } from "../src/supervisedEditsApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.clearAllMocks(); vi.useRealTimers(); });
const target = { project_id: "project", feature_id: "feature" };

function edit(overrides: Partial<SupervisedEdit> = {}): SupervisedEdit {
  return {
    id: "edit-1", revision: "rev-1", kind: "diff-review", path: "src/invoice.ts", tool: "edit",
    is_new_file: false, agent_reason: null, diff_error: null, old_snippet: null, new_snippet: null,
    requested_at: 1_790_000_000, answered: false, unavailable: null,
    effects: {
      approve: "The agent writes this change.",
      reject: "The agent does not write this change and receives your feedback.",
      cancel: "The agent does not write this change and is told you cancelled it.",
      feedback_reaches_agent: true,
    },
    diff: { path: "src/invoice.ts", old_path: "src/invoice.ts", status: "modified", additions: 1, deletions: 1, is_binary: false, patch: "",
      hunks: [{ header: "@@ -1,2 +1,2 @@", lines: [
        { kind: "context", text: " export function total() {", old_line: 1, new_line: 1 },
        { kind: "removed", text: "-return subtotal;", old_line: 2, new_line: null },
        { kind: "added", text: "+return Math.round(subtotal * 100) / 100;", old_line: null, new_line: 2 },
      ] }] },
    ...overrides,
  };
}
const view = (edits: SupervisedEdit[]): SupervisedEditsView => ({ target, feature_name: "Round totals", edits });

function mount(onClose = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  render(<QueryClientProvider client={client}><SupervisedEditsPanel target={target} onClose={onClose} /></QueryClientProvider>);
  return onClose;
}
const responds = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "supervised_edit_respond");

it("shows the pending diff and sends an approval only after explicit confirmation, once", async () => {
  let current = view([edit({ agent_reason: "Round to cents" })]);
  let release!: () => void;
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "supervised_edits_load") return current;
    if (command === "supervised_edit_respond") {
      await new Promise<void>((resolve) => { release = resolve; });
      current = view([{ ...edit(), answered: true }]);
      return { message: "Approved the edit to src/invoice.ts", view: current };
    }
    return [];
  });
  mount();
  expect(await screen.findByText("+return Math.round(subtotal * 100) / 100;")).toBeTruthy();
  expect(screen.getByText("Round to cents")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Approve edit" }));
  expect(responds()).toHaveLength(0);
  const dialog = screen.getByRole("alertdialog", { name: "Confirm answer" });
  expect(within(dialog).getByText(/The agent writes this change\./)).toBeTruthy();
  fireEvent.click(within(dialog).getByRole("button", { name: "Send approval" }));
  fireEvent.click(within(dialog).getByRole("button", { name: "Send approval" }));
  await waitFor(() => expect(responds()).toHaveLength(1));
  expect(responds()[0][1]).toEqual({ target, editId: "edit-1", revision: "rev-1", decision: { kind: "approve" } });
  await act(async () => release());
  expect(await screen.findByText(/Approved the edit to src\/invoice.ts/)).toBeTruthy();
  expect(screen.getAllByText("Answer sent. Waiting for the agent to pick it up.")).toHaveLength(1);
  expect((screen.getByRole("button", { name: "Approve edit" }) as HTMLButtonElement).disabled).toBe(true);
  expect(responds()).toHaveLength(1);
});

it("keeps rejection feedback after a refused answer and reports the conflict", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "supervised_edits_load") return view([edit()]);
    if (command === "supervised_edit_respond") throw { kind: "conflict", message: "This edit changed after you reviewed it" };
    return [];
  });
  mount();
  await screen.findByText("-return subtotal;");
  const feedback = screen.getByRole("textbox", { name: "Feedback for the agent" }) as HTMLTextAreaElement;
  fireEvent.change(feedback, { target: { value: "  Keep raw totals; café 🧾  " } });
  fireEvent.click(screen.getByRole("button", { name: "Reject edit" }));
  const dialog = screen.getByRole("alertdialog", { name: "Confirm answer" });
  expect(within(dialog).getByText("Keep raw totals; café 🧾")).toBeTruthy();
  fireEvent.click(within(dialog).getByRole("button", { name: "Send rejection" }));
  expect(await screen.findByRole("alert")).toBeTruthy();
  expect(screen.getByRole("alert").textContent).toContain("changed after you reviewed it");
  expect(responds()[0][1]).toMatchObject({ decision: { kind: "reject", feedback: "Keep raw totals; café 🧾" } });
  expect((screen.getByRole("textbox", { name: "Feedback for the agent" }) as HTMLTextAreaElement).value)
    .toBe("  Keep raw totals; café 🧾  ");
});

it("drops a confirmation whose revision changed and says when the edit is gone", async () => {
  let current = view([edit()]);
  vi.mocked(invoke).mockImplementation(async (command) => command === "supervised_edits_load" ? current : []);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  render(<QueryClientProvider client={client}><SupervisedEditsPanel target={target} onClose={vi.fn()} /></QueryClientProvider>);
  await screen.findByText("-return subtotal;");
  fireEvent.click(screen.getByRole("button", { name: "Cancel edit" }));
  expect(screen.getByRole("alertdialog", { name: "Confirm answer" })).toBeTruthy();
  current = view([edit({ revision: "rev-2" })]);
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  expect(await screen.findByText(/This edit changed while you were confirming/)).toBeTruthy();
  expect(screen.queryByRole("alertdialog", { name: "Confirm answer" })).toBeNull();
  fireEvent.change(screen.getByRole("textbox", { name: "Feedback for the agent" }), { target: { value: "unsent" } });
  current = view([]);
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  expect(await screen.findByText(/is no longer waiting for review/)).toBeTruthy();
  expect(screen.getByText(/Your unsent feedback was not delivered/)).toBeTruthy();
  expect(screen.getByText("No edits are waiting for review")).toBeTruthy();
  expect(responds()).toHaveLength(0);
});

it("asks before discarding unsent feedback and states OpenCode's semantics", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => command === "supervised_edits_load" ? view([edit({
    kind: "change-reason",
    effects: { approve: "a", reject: "r", cancel: "OpenCode treats cancel as a skip: the agent writes this change without recording a reason.", feedback_reaches_agent: false },
  })]) : []);
  const onClose = mount();
  await screen.findByText("-return subtotal;");
  expect(screen.getByText(/OpenCode does not forward rejection feedback/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Cancel edit" }));
  expect(screen.getByText(/OpenCode treats cancel as a skip/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Back" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Feedback for the agent" }), { target: { value: "draft" } });
  fireEvent.click(screen.getAllByRole("button", { name: "Close" })[0]);
  expect(onClose).not.toHaveBeenCalled();
  const prompt = screen.getByRole("alertdialog", { name: "Discard unsent feedback" });
  fireEvent.click(within(prompt).getByRole("button", { name: "Keep editing" }));
  expect((screen.getByRole("textbox", { name: "Feedback for the agent" }) as HTMLTextAreaElement).value).toBe("draft");
  fireEvent.click(screen.getAllByRole("button", { name: "Close" })[0]);
  fireEvent.click(screen.getByRole("button", { name: "Discard and close" }));
  expect(onClose).toHaveBeenCalledTimes(1);
  expect(responds()).toHaveLength(0);
});

it("announces only edits that arrive after the first poll, once each", async () => {
  let counts: PendingEditCount[] = [{ project_id: "project", feature_id: "feature", feature_name: "Round totals", count: 1, first_id: "old", first_path: "a.ts" }];
  vi.mocked(invoke).mockImplementation(async (command) => command === "supervised_edit_counts" ? counts : []);
  const pushToast = vi.fn();
  const onOpen = vi.fn();
  let latest: Record<string, number> = {};
  function Harness() { latest = usePendingEdits(pushToast, onOpen); return null; }
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  render(<QueryClientProvider client={client}><Harness /></QueryClientProvider>);
  await waitFor(() => expect(latest).toEqual({ feature: 1 }));
  expect(pushToast).not.toHaveBeenCalled();
  counts = [{ ...counts[0], count: 2 }, { project_id: "project", feature_id: "other", feature_name: "Docs", count: 1, first_id: "new", first_path: "README.md" }];
  await act(async () => { await client.refetchQueries({ queryKey: ["supervised-edit-counts"] }); });
  await waitFor(() => expect(pushToast).toHaveBeenCalledTimes(1));
  expect(pushToast.mock.calls[0][0].message).toBe("Docs: the agent wants to change README.md.");
  pushToast.mock.calls[0][0].action.onClick();
  expect(onOpen).toHaveBeenCalledWith({ project_id: "project", feature_id: "other" });
  await act(async () => { await client.refetchQueries({ queryKey: ["supervised-edit-counts"] }); });
  expect(pushToast).toHaveBeenCalledTimes(1);
  expect(latest).toEqual({ feature: 2, other: 1 });
});


it.each(["Close", "Escape"])("protects feedback on %s while a new context query is loading", async (closeAction) => {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "supervised_edits_load") {
      if ((args as { context: string }).context !== "standard") return new Promise(() => {});
      return view([edit()]);
    }
    return [];
  });
  const onClose = mount();
  await screen.findByText("-return subtotal;");
  fireEvent.change(screen.getByRole("textbox", { name: "Feedback for the agent" }), { target: { value: "keep draft" } });
  fireEvent.change(screen.getByLabelText("Context"), { target: { value: "expanded" } });
  expect(await screen.findByText(/Loading pending edits/)).toBeTruthy();
  if (closeAction === "Close") fireEvent.click(screen.getAllByRole("button", { name: "Close" })[0]);
  else fireEvent.keyDown(document, { key: "Escape" });
  expect(onClose).not.toHaveBeenCalled();
  const prompt = screen.getByRole("alertdialog", { name: "Discard unsent feedback" });
  expect(prompt.textContent).toContain("src/invoice.ts");
  fireEvent.click(within(prompt).getByRole("button", { name: "Keep editing" }));
  fireEvent.change(screen.getByLabelText("Context"), { target: { value: "standard" } });
  expect((await screen.findByRole("textbox", { name: "Feedback for the agent" }) as HTMLTextAreaElement).value).toBe("keep draft");
  expect(responds()).toHaveLength(0);
});
