// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import UserPromptEditor from "../src/UserPromptEditor";
import type { UserPromptEditorData } from "../src/api";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
const snapshot: UserPromptEditorData = { id: "original", revision: "raw-revision", name: "Repair", body: "Fix {{area}}", tags: "bug", description: "Keep description", configured_fields: 2 };
function mount(existing: UserPromptEditorData | null = snapshot) {
  const onClose = vi.fn(), onSaved = vi.fn();
  render(<UserPromptEditor snapshot={existing} seedBody="Unsent composer 世界" onClose={onClose} onSaved={onSaved} />);
  return { onClose, onSaved };
}
it("retains edited fields after stale save and failed reload, then explicitly discards for a fresh revision", async () => {
  const { onClose, onSaved } = mount();
  vi.mocked(invoke).mockRejectedValue({ kind: "conflict", message: "Changed elsewhere. Keep your draft." });
  fireEvent.change(screen.getByRole("textbox", { name: /^Template body/ }), { target: { value: "Local unsaved body" } });
  fireEvent.click(screen.getByRole("button", { name: "Save User template" }));
  await screen.findByText("Changed elsewhere. Keep your draft.");
  expect((screen.getByRole("textbox", { name: /^Template body/ }) as HTMLTextAreaElement).value).toBe("Local unsaved body");
  expect(invoke).toHaveBeenCalledWith("prompt_user_save", { draft: { id: "original", revision: "raw-revision", name: "Repair", body: "Local unsaved body", tags: "bug" } });
  expect(onSaved).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Reload template" }));
  expect(screen.getByRole("dialog", { name: "Discard prompt edits" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
  await screen.findByText("Changed elsewhere. Keep your draft.");
  expect((screen.getByRole("textbox", { name: /^Template body/ }) as HTMLTextAreaElement).value).toBe("Local unsaved body");
  vi.mocked(invoke).mockResolvedValue({ ...snapshot, revision: "new-revision", body: "External version" });
  fireEvent.click(screen.getByRole("button", { name: "Reload template" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
  await waitFor(() => expect((screen.getByRole("textbox", { name: /^Template body/ }) as HTMLTextAreaElement).value).toBe("External version"));
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(onClose).toHaveBeenCalledTimes(1);
});
it("protects close and never changes the supplied composer body during cancellation", () => {
  const { onClose } = mount(null);
  expect((screen.getByRole("textbox", { name: /^Template body/ }) as HTMLTextAreaElement).value).toBe("Unsent composer 世界");
  fireEvent.change(screen.getByLabelText("Template name"), { target: { value: "My prompt" } });
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  fireEvent.keyDown(document, { key: "Escape" });
  expect(onClose).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
  expect(onClose).toHaveBeenCalledTimes(1);
  expect(invoke).not.toHaveBeenCalled();
});
it("locks pending saves against double submission, Escape and cancellation, keeping failures retryable", async () => {
  let reject!: (reason: unknown) => void;
  vi.mocked(invoke).mockImplementation(() => new Promise((_resolve, fail) => { reject = fail; }));
  const { onClose, onSaved } = mount(null);
  fireEvent.change(screen.getByLabelText("Template name"), { target: { value: "Composer" } });
  const button = screen.getByRole("button", { name: "Save User template" });
  fireEvent.click(button); fireEvent.click(button);
  fireEvent.keyDown(document, { key: "Escape" });
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(invoke).toHaveBeenCalledTimes(1); expect(onClose).not.toHaveBeenCalled();
  await act(async () => reject({ kind: "operation_failed", message: "Database busy" }));
  await screen.findByText("Database busy");
  expect((screen.getByRole("textbox", { name: /^Template body/ }) as HTMLTextAreaElement).value).toBe("Unsent composer 世界");
  vi.mocked(invoke).mockResolvedValue("new-id"); fireEvent.click(button);
  await waitFor(() => expect(onSaved).toHaveBeenCalledWith("new-id"));
  expect(invoke).toHaveBeenCalledTimes(2);
});
