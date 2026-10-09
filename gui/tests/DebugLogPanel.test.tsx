// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import DebugLogPanel from "../src/DebugLogPanel";
import type { DebugLogView } from "../src/debugLogApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.resetAllMocks(); });
const view: DebugLogView = {
  limit: 1000, shared_history: true,
  entries: [
    { timestamp: "2026-10-09T10:00:00Z", level: "DEBUG", context: "tmux", message: "Pane attached" },
    { timestamp: "2026-10-09T10:01:00Z", level: "INFO", context: "amf", message: "Ready" },
    { timestamp: "2026-10-09T10:02:00Z", level: "WARN", context: "sync", message: "café\n<script>alert(1)</script>" },
    { timestamp: "2026-10-09T10:03:00Z", level: "ERROR", context: "worktree", message: "Could not create checkout" },
  ],
};
function mount(cached?: DebugLogView) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  if (cached) client.setQueryData(["debug-log"], cached);
  const onClose = vi.fn();
  render(<QueryClientProvider client={client}><DebugLogPanel onClose={onClose} /></QueryClientProvider>);
  return { onClose };
}
const refresh = () => fireEvent.click(screen.getByRole("button", { name: "Refresh log" }));

it("renders chronological shared history as text and filters by exact level plus context or message", async () => {
  vi.mocked(invoke).mockResolvedValue(view);
  mount();
  const list = await screen.findByRole("list", { name: "Log entries" });
  expect(within(list).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
    expect.stringContaining("Pane attached"), expect.stringContaining("Ready"),
    expect.stringContaining("<script>alert(1)</script>"), expect.stringContaining("Could not create checkout"),
  ]);
  expect(list.querySelector("script")).toBeNull();
  fireEvent.change(screen.getByLabelText("Level"), { target: { value: "WARN" } });
  fireEvent.change(screen.getByLabelText("Search context or message"), { target: { value: "CAFÉ" } });
  expect(within(list).getAllByRole("listitem")).toHaveLength(1);
  fireEvent.change(screen.getByLabelText("Search context or message"), { target: { value: "SYNC" } });
  expect(within(list).getAllByRole("listitem")).toHaveLength(1);
  fireEvent.change(screen.getByLabelText("Level"), { target: { value: "ERROR" } });
  expect(screen.getByText("No entries match these filters.")).toBeTruthy();
  expect(vi.mocked(invoke).mock.calls).toEqual([["debug_log_load"]]);
});

it("refreshes without reopening and retains the viewer filters", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(view).mockResolvedValueOnce({ ...view,
    entries: [...view.entries, { ...view.entries[2], message: "New sync warning" }],
  });
  mount();
  await screen.findByText("Pane attached");
  fireEvent.change(screen.getByLabelText("Level"), { target: { value: "WARN" } });
  fireEvent.change(screen.getByLabelText("Search context or message"), { target: { value: "sync" } });
  refresh();
  expect(await screen.findByText("New sync warning")).toBeTruthy();
  expect(screen.queryByText("Pane attached")).toBeNull();
  expect(screen.getByText(/Showing 2 of 5/)).toBeTruthy();
});

it("reports refresh failure as previous history and permits retry", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(view).mockRejectedValueOnce({ kind: "internal", message: "Database busy" })
    .mockResolvedValueOnce({ ...view, entries: [] });
  mount();
  await screen.findByText("Pane attached");
  refresh();
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", expect.stringContaining("Database busy"));
  expect(screen.getByText("Showing the previous load. Refresh to try again.")).toBeTruthy();
  expect(screen.getByText("Pane attached")).toBeTruthy();
  refresh();
  expect(await screen.findByText("No log entries yet.")).toBeTruthy();
  expect(screen.queryByRole("alert")).toBeNull();
});

it("hides cached history when this opening fails and loads it only after retry", async () => {
  vi.mocked(invoke).mockRejectedValueOnce({ kind: "internal", message: "Read failed" }).mockResolvedValueOnce(view);
  mount(view);
  expect(await screen.findByRole("alert")).toBeTruthy();
  expect(screen.queryByText("Pane attached")).toBeNull();
  refresh();
  expect(await screen.findByText("Pane attached")).toBeTruthy();
});

it("states when only process-local history is available and closes with Escape", async () => {
  vi.mocked(invoke).mockResolvedValue({ ...view, entries: [], shared_history: false });
  const { onClose } = mount();
  expect(await screen.findByText(/Shared database unavailable/)).toBeTruthy();
  fireEvent.keyDown(document, { key: "Escape" });
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
});
