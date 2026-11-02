// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import NeedsAttentionPanel, { AttentionRow, NeedsAttentionView } from "../src/NeedsAttentionPanel";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const target = { project_id: "p", feature_id: "f", session_id: "s" };
const row = (over: Partial<AttentionRow> = {}): AttentionRow => ({
  id: "/w/.claude/notifications/a.json", modified_ms: 1000, project_id: "p", feature_id: "f",
  project_name: "Proj", feature_name: "feat", kind: "input-request", reason: "Waiting for input",
  message: "Which database?", age: "5m ago", age_secs: 300, stale: false, session: target,
  jump_note: null, dismissable: true, dismiss_note: null, ...over,
});
const view = (rows: AttentionRow[], limits = ["Labels come from a running TUI."]): NeedsAttentionView =>
  ({ rows, hidden_stopped: 0, hidden_resumed: 0, limits });
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach(c => c.clear()); vi.resetAllMocks(); });
function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(client);
  const onJump = vi.fn(); const onClose = vi.fn();
  render(<QueryClientProvider client={client}><NeedsAttentionPanel onJump={onJump} onClose={onClose} /></QueryClientProvider>);
  return { onJump, onClose };
}
it("shows reason, age, message and the TUI-only limit, and jumps by exact target without any write", async () => {
  vi.mocked(invoke).mockResolvedValue(view([row()]));
  const { onJump, onClose } = mount();
  expect(await screen.findByText("Waiting for input")).toBeTruthy();
  expect(screen.getByText("5m ago")).toBeTruthy();
  expect(screen.getByText("Which database?")).toBeTruthy();
  expect(screen.getByRole("note").textContent).toContain("running TUI");
  fireEvent.click(screen.getByRole("button", { name: "Open session for feat" }));
  await waitFor(() => expect(onJump).toHaveBeenCalledWith(target));
  expect(onClose).toHaveBeenCalled();
  expect(vi.mocked(invoke).mock.calls.every(([c]) => c === "needs_attention_load")).toBe(true);
});
it("disables dismiss with a stated reason for edit reviews and disables jump when unmatched", async () => {
  vi.mocked(invoke).mockResolvedValue(view([row({ kind: "diff-review", reason: "Edit waiting for review", dismissable: false,
    dismiss_note: "Answer it in Supervised edits.", session: null, jump_note: "could not be matched to a feature" })]));
  mount();
  expect(await screen.findByText("Answer it in Supervised edits.")).toBeTruthy();
  expect(screen.getByText(/could not be matched/)).toBeTruthy();
  expect(screen.getByRole("button", { name: /Dismiss feat/ }).hasAttribute("disabled")).toBe(true);
  expect(screen.getByRole("button", { name: /Open session for feat/ }).hasAttribute("disabled")).toBe(true);
});
it("dismisses by stable id plus modified time and replaces the list with the refreshed one", async () => {
  vi.mocked(invoke).mockImplementation(async (command) =>
    command === "needs_attention_dismiss" ? view([]) : view([row()]));
  mount();
  fireEvent.click(await screen.findByRole("button", { name: /Dismiss feat/ }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("needs_attention_dismiss",
    { id: "/w/.claude/notifications/a.json", modifiedMs: 1000 }));
  expect(await screen.findByText("Nothing listed needs attention.")).toBeTruthy();
});
it("shows a refusal from the backend and keeps the row", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "needs_attention_dismiss") throw { kind: "conflict", message: "That request changed since the list loaded." };
    return view([row()]);
  });
  mount();
  fireEvent.click(await screen.findByRole("button", { name: /Dismiss feat/ }));
  expect((await screen.findByRole("alert")).textContent).toContain("changed since the list loaded");
  expect(screen.getByText("Which database?")).toBeTruthy();
});
