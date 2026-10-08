// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import PrScreenshotsPanel from "../src/PrScreenshotsPanel";
import type { RemoteListing } from "../src/screenshotsApi";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
const base: RemoteListing = { request_id: "request", items: [{ key: "image", caption: "Ready", provenance: ["Description · repo @ pinned-commit"] }], galleries: [], issues: [], runs: [{ id: 10, attempt: 2, name: "Visual", head_sha: "current", status: "completed", conclusion: "failure", created_at: "2026-10-07T00:00:00Z" }], selected_run: 10, run_page: 1, more_runs: true };
function mock() { vi.mocked(invoke).mockImplementation(async (command, args) => {
  if (command === "screenshots_remote_list") return { ...base, request_id: (args as { requestId: string }).requestId };
  if (command === "screenshots_remote_image") return { data_url: "data:image/png;base64,AA==", width: 4, height: 3 };
  if (command === "screenshots_remote_close") return;
  throw new Error(command);
}); }
it("browses pinned evidence and exposes older run pages without resetting the surrounding review", async () => {
  mock(); const close = vi.fn(); render(<PrScreenshotsPanel workflowId="triage" prNumber={7} headSha="current" onClose={close} />);
  fireEvent.click(await screen.findByRole("button", { name: "View screenshot Ready" }));
  expect(await screen.findByText("Description · repo @ pinned-commit")).toBeTruthy(); fireEvent.click(screen.getByRole("button", { name: "Original size" }));
  fireEvent.keyDown(window, { key: "Escape" }); expect(close).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Older runs page" }));
  await waitFor(() => expect(vi.mocked(invoke).mock.calls.some(([command, args]) => command === "screenshots_remote_list" && (args as { runPage: number }).runPage === 2)).toBe(true));
});
it("cancels the known pending request on close and rejects a late result from a prior PR", async () => {
  let resolve!: (listing: RemoteListing) => void; const delayed = new Promise<RemoteListing>((done) => { resolve = done; }); mock(); const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((command, args, options) => command === "screenshots_remote_list" && (args as { workflowId: string }).workflowId === "old" ? delayed : original(command, args, options));
  const view = render(<PrScreenshotsPanel workflowId="old" prNumber={1} headSha="old-head" onClose={() => {}} />);
  const old = vi.mocked(invoke).mock.calls.find(([c]) => c === "screenshots_remote_list")![1] as { requestId: string };
  view.rerender(<PrScreenshotsPanel workflowId="new" prNumber={2} headSha="new-head" onClose={() => {}} />);
  expect(await screen.findByRole("button", { name: "View screenshot Ready" })).toBeTruthy();
  expect(invoke).toHaveBeenCalledWith("screenshots_remote_close", { requestId: old.requestId }); resolve({ ...base, request_id: old.requestId, items: [{ key: "stale", caption: "Stale PR evidence", provenance: [] }] });
  await waitFor(() => expect(screen.queryByText("Stale PR evidence")).toBeNull());
});
