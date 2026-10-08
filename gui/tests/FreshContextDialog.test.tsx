// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import FreshContextDialog from "../src/FreshContextDialog";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const target = { project_id: "p", feature_id: "f", session_id: "s" };
const preview = { revision: "r1", prompt: "Read AMF_PLAN.md. Continue carefully.", label: "Fresh Context" };
afterEach(() => { cleanup(); vi.resetAllMocks(); });
function mount() {
  const onCreated = vi.fn(), onClose = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><FreshContextDialog target={target} onCreated={onCreated} onClose={onClose} /></QueryClientProvider>);
  return { onCreated, onClose };
}
it("edits the seed and opens the returned target with an unsent draft", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd) => cmd === "fresh_context_preview" ? preview :
    { target: { ...target, session_id: "new" }, draft_prompt: "Edited continuation" });
  const handlers = mount();
  const input = await screen.findByRole("textbox", { name: "Continuation prompt" });
  expect((input as HTMLTextAreaElement).value).toBe(preview.prompt);
  fireEvent.change(input, { target: { value: "Edited continuation" } });
  fireEvent.click(screen.getByRole("button", { name: "Start fresh context" }));
  await waitFor(() => expect(handlers.onCreated).toHaveBeenCalledWith({ ...target, session_id: "new" }, "Edited continuation"));
  expect(invoke).toHaveBeenCalledWith("fresh_context_start", { target, request: { revision: "r1", prompt: "Edited continuation", approved: false } });
  expect(handlers.onClose).toHaveBeenCalledOnce();
});
it("protects edited text on cancellation without starting", async () => {
  vi.mocked(invoke).mockResolvedValue(preview);
  const handlers = mount();
  fireEvent.change(await screen.findByRole("textbox"), { target: { value: "My work" } });
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.getByRole("alertdialog", { name: "Discard continuation draft" })).toBeTruthy();
  expect(handlers.onClose).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe("My work");
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard draft" }));
  expect(handlers.onClose).toHaveBeenCalledOnce();
  expect(vi.mocked(invoke).mock.calls.map(([cmd]) => cmd)).toEqual(["fresh_context_preview"]);
});
it("retains a stale draft and reloads its revision before retry", async () => {
  let loads = 0;
  vi.mocked(invoke).mockImplementation(async (cmd) => {
    if (cmd === "fresh_context_preview") return { ...preview, revision: `r${++loads}` };
    throw { kind: "conflict", message: "Source changed" };
  });
  mount();
  fireEvent.change(await screen.findByRole("textbox"), { target: { value: "Retain me" } });
  fireEvent.click(screen.getByRole("button", { name: "Start fresh context" }));
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Source changed");
  fireEvent.click(screen.getByRole("button", { name: "Reload context (keep draft)" }));
  await waitFor(() => expect(loads).toBe(2));
  expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe("Retain me");
  fireEvent.click(screen.getByRole("button", { name: "Start fresh context" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("fresh_context_start", { target, request: { revision: "r2", prompt: "Retain me", approved: false } }));
});
it("requires resource approval and blocks repeated pending starts", async () => {
  let launches = 0;
  let finish!: (value: unknown) => void;
  vi.mocked(invoke).mockImplementation(async (cmd) => {
    if (cmd === "fresh_context_preview") return preview;
    if (++launches === 1) throw { kind: "needs_approval", message: "Too many agents" };
    return new Promise((resolve) => { finish = resolve; });
  });
  const handlers = mount();
  await screen.findByRole("textbox");
  fireEvent.click(screen.getByRole("button", { name: "Start fresh context" }));
  await screen.findByRole("alertdialog", { name: "Resource warning" });
  fireEvent.click(screen.getByRole("button", { name: "Start anyway" }));
  fireEvent.click(screen.getByRole("button", { name: "Start anyway" }));
  expect(launches).toBe(2);
  expect(invoke).toHaveBeenCalledWith("fresh_context_start", { target, request: { revision: "r1", prompt: preview.prompt, approved: true } });
  finish({ target, draft_prompt: preview.prompt });
  await waitFor(() => expect(handlers.onCreated).toHaveBeenCalledOnce());
});
