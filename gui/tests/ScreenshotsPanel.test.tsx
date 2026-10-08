// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import ScreenshotsPanel from "../src/ScreenshotsPanel";
import type { EvidenceItem, EvidenceListing, EvidenceOwner } from "../src/screenshotsApi";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
const owner: EvidenceOwner = { version: 1, scope_id: "scope", project_id: "project", feature_id: "feature", session_id: "session", project_name: "Project", feature_name: "Feature", session_label: "Codex 1", workdir: "/repo", is_worktree: false, created_at: "2026-10-07T00:00:00Z" };
const item: EvidenceItem = { key: "scope:ready", scope_id: "scope", image_id: "ready", file: "ready.png", sha256: "aaa", caption: "Ready state", captured_at: "2026-10-07T00:00:00Z", owner };
const listing: EvidenceListing = { items: [item], owners: [owner], issues: [], truncated: false };
function mock() { vi.mocked(invoke).mockImplementation(async (command) => {
  if (command === "screenshots_list") return listing;
  if (command === "screenshots_image") return { data_url: "data:image/png;base64,AA==", width: 4, height: 3 };
  if (command === "screenshots_cleanup_scopes") return [[owner, false]];
  if (command === "screenshots_cleanup") return;
  if (command === "screenshots_changed") return false;
  throw new Error(command);
}); }
it("opens attributed evidence, returns to its thumbnail and cleans only a selected scope", async () => {
  mock(); const close = vi.fn(); render(<ScreenshotsPanel onClose={close} />);
  const open = await screen.findByRole("button", { name: "View screenshot Ready state" }); open.focus(); fireEvent.click(open);
  expect(await screen.findByRole("region", { name: "Screenshot viewer" })).toBeTruthy();
  expect(screen.getByText(/Feature · Codex 1 · session/)).toBeTruthy(); fireEvent.click(screen.getByRole("button", { name: "Original size" }));
  fireEvent.keyDown(window, { key: "Escape" }); expect(close).not.toHaveBeenCalled(); expect(await screen.findByRole("button", { name: "View screenshot Ready state" })).toBeTruthy();
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: "View screenshot Ready state" })));
  fireEvent.click(screen.getByRole("button", { name: "Screenshot cleanup…" })); fireEvent.click(await screen.findByRole("button", { name: "Clean up this scope…" }));
  expect(vi.mocked(invoke).mock.calls.filter(([c]) => c === "screenshots_cleanup")).toHaveLength(0);
  fireEvent.click(screen.getByRole("button", { name: "Delete screenshots" })); await waitFor(() => expect(invoke).toHaveBeenCalledWith("screenshots_cleanup", { scopeId: "scope" }));
});
it("rejects a delayed previous-session listing and an image result after unmount", async () => {
  let resolveOld!: (value: EvidenceListing) => void;
  const delayed = new Promise<EvidenceListing>((resolve) => { resolveOld = resolve; }); mock(); const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((command, args, options) => command === "screenshots_list" && (args as { selection: { session_id: string } }).selection.session_id === "old" ? delayed : original(command, args, options));
  const view = render(<ScreenshotsPanel target={{ project_id: "project", feature_id: "feature" }} sessionId="old" onClose={() => {}} />);
  // Change the session filter while the original scan is in flight.
  fireEvent.change(screen.getByRole("combobox"), { target: { value: "" } }); expect(await screen.findByText("Ready state")).toBeTruthy();
  resolveOld({ ...listing, items: [{ ...item, caption: "Old session evidence" }] });
  await waitFor(() => expect(screen.queryByText("Old session evidence")).toBeNull()); view.unmount();
});

it("clears cached images when the current feature disappears during refresh", async () => {
  mock(); render(<ScreenshotsPanel target={{ project_id: "project", feature_id: "feature" }} onClose={() => {}} />);
  expect(await screen.findByText("Ready state")).toBeTruthy();
  const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((command, args, options) => command === "screenshots_list" ? Promise.reject({ kind: "not_found", message: "Feature deleted" }) : original(command, args, options));
  fireEvent.click(screen.getByRole("button", { name: "Refresh screenshots" }));
  expect(await screen.findByText("Feature deleted")).toBeTruthy();
  expect(screen.queryByText("Ready state")).toBeNull();
});
