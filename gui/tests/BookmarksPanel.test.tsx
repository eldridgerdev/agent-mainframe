// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import BookmarksPanel, { BookmarkRow } from "../src/BookmarksPanel";
import type { Project } from "../src/api";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const target = { project_id: "p", feature_id: "f", session_id: "s" };
const row: BookmarkRow = { slot: 1, target, label: "Project / Feature / Terminal", stale: false };
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach(client => client.clear()); vi.resetAllMocks(); });
function mount(cached?: BookmarkRow[]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(client);
  if (cached) client.setQueryData(["bookmarks"], cached);
  const onOpen = vi.fn(); const onClose = vi.fn();
  const projects = [{ id: "p", name: "Project", features: [{ id: "f", name: "Feature", sessions: [{ id: "s", label: "Terminal" }] }] }] as Project[];
  render(<QueryClientProvider client={client}><BookmarksPanel projects={projects} onOpen={onOpen} onClose={onClose} /></QueryClientProvider>);
  return { onOpen, onClose };
}
it("resolves a stable target before navigation without a launch command", async () => {
  vi.mocked(invoke).mockImplementation(async command => command === "bookmarks_resolve" ? target : [row]);
  const { onOpen, onClose } = mount();
  fireEvent.click(await screen.findByRole("button", { name: "1. Project / Feature / Terminal" }));
  await waitFor(() => expect(onOpen).toHaveBeenCalledWith(target)); expect(onClose).toHaveBeenCalledOnce();
  expect(vi.mocked(invoke).mock.calls).toContainEqual(["bookmarks_resolve", { target }]);
  expect(vi.mocked(invoke).mock.calls.every(([command]) => command.startsWith("bookmarks_"))).toBe(true);
});
it("adds selected sessions and removes by stable target rather than slot", async () => {
  vi.mocked(invoke).mockResolvedValue([row]); mount(); await screen.findByRole("button", { name: "Remove bookmark 1" });
  fireEvent.change(screen.getByLabelText("Session to bookmark"), { target: { value: '["p","f","s"]' } });
  fireEvent.click(screen.getByRole("button", { name: "Bookmark session" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("bookmarks_add", { target }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Remove bookmark 1" }).hasAttribute("disabled")).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Remove bookmark 1" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("bookmarks_remove", { target }));
});
it("keeps stale-target errors visible and refuses stale cached rows after a failed opening", async () => {
  vi.mocked(invoke).mockRejectedValue({ kind: "internal", message: "Database busy" }); mount([row]);
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", expect.stringContaining("Database busy"));
  expect(screen.queryByRole("button", { name: "1. Project / Feature / Terminal" })).toBeNull();
});
it("retains picker on stale resolution and refreshes after pruning", async () => {
  vi.mocked(invoke).mockResolvedValueOnce([row]).mockRejectedValueOnce({ kind: "conflict", message: "Stale bookmark removed" }).mockResolvedValue([]);
  const { onOpen, onClose } = mount(); fireEvent.click(await screen.findByRole("button", { name: "1. Project / Feature / Terminal" }));
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Stale bookmark removed");
  expect(await screen.findByText("No bookmarked sessions yet.")).toBeTruthy();
  expect(onOpen).not.toHaveBeenCalled(); expect(onClose).not.toHaveBeenCalled();
});
it("guards duplicate writes and dismissal while a bookmark mutation is pending", async () => {
  let finish!: () => void;
  const pending = new Promise<void>(resolve => { finish = resolve; });
  vi.mocked(invoke).mockImplementation(async command => command === "bookmarks_remove" ? pending : [row]);
  const { onClose } = mount();
  const remove = await screen.findByRole("button", { name: "Remove bookmark 1" });
  fireEvent.click(remove); fireEvent.click(remove);
  fireEvent.click(screen.getByText("Close", { selector: "button" }));
  fireEvent.keyDown(document, { key: "Escape" });
  expect(onClose).not.toHaveBeenCalled();
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "bookmarks_remove")).toHaveLength(1);
  finish();
  await waitFor(() => expect(screen.getByText("Close", { selector: "button" }).hasAttribute("disabled")).toBe(false));
  fireEvent.click(screen.getByText("Close", { selector: "button" })); expect(onClose).toHaveBeenCalledOnce();
});
it("labels retained results after refresh failure and disables stale actions until retry", async () => {
  vi.mocked(invoke).mockResolvedValueOnce([row]).mockRejectedValueOnce({ kind: "internal", message: "Database busy" }).mockResolvedValue([row]);
  mount(); await screen.findByRole("button", { name: "Remove bookmark 1" });
  fireEvent.click(screen.getByRole("button", { name: "Refresh bookmarks" }));
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Showing the previous load");
  expect(screen.getByRole("button", { name: "Remove bookmark 1" }).hasAttribute("disabled")).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Refresh bookmarks" }));
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  expect(screen.getByRole("button", { name: "Remove bookmark 1" }).hasAttribute("disabled")).toBe(false);
});
