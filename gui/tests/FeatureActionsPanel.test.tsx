// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import FeatureActionsPanel, { FeatureActionsView } from "../src/FeatureActionsPanel";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const target = { project_id: "p", feature_id: "f" };
const view: FeatureActionsView = { name: "Feature", nickname: null, ready: false, summary: null,
  summary_generating: false, sessions: [{ id: "s", label: "Claude" }], summary_outcome: null };
const notice = { prompt_id: "session.summary", title: "Session summary", harness: "claude", harness_name: "Claude",
  template: "Summarise {{recent_lines}}", revision: "rev1" };
afterEach(() => { cleanup(); vi.resetAllMocks(); });
function mount(handler: (command: string, args: any) => unknown) {
  vi.mocked(invoke).mockImplementation(async (command, args) => handler(command, args) as never);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onChanged = vi.fn();
  render(<QueryClientProvider client={client}><FeatureActionsPanel target={target} onClose={vi.fn()} onChanged={onChanged} /></QueryClientProvider>);
  return { onChanged };
}
const calls = (name: string) => vi.mocked(invoke).mock.calls.filter(([command]) => command === name);

it("renames with the value it last showed and echoes it back", async () => {
  mount((command) => command === "feature_rename" ? { ...view, nickname: "Shiny" } : view);
  const input = await screen.findByRole("textbox", { name: "Nickname" });
  fireEvent.change(input, { target: { value: "Shiny" } });
  fireEvent.click(screen.getByRole("button", { name: "Save nickname" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("feature_rename", { target, expected: null, nickname: "Shiny" }));
});

it("shows a refused stale rename instead of retargeting", async () => {
  mount((command) => { if (command === "session_rename") throw { kind: "conflict", message: "The session name changed since you opened this. Reload and try again." }; return view; });
  const input = await screen.findByLabelText("Name of session Claude");
  fireEvent.change(input, { target: { value: "Main" } });
  fireEvent.click(screen.getByRole("button", { name: "Rename session Claude" }));
  expect((await screen.findByRole("alert")).textContent).toContain("changed since you opened this");
  expect(calls("session_rename")[0][1]).toEqual({ target: { ...target, session_id: "s" }, expected: "Claude", label: "Main" });
});

it("toggles ready against the value it showed", async () => {
  mount((command) => command === "feature_set_ready" ? { ...view, ready: true } : view);
  fireEvent.click(await screen.findByRole("button", { name: "Mark ready" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("feature_set_ready", { target, expected: false }));
  expect(await screen.findByRole("button", { name: "Mark not ready" })).toBeTruthy();
});

it("shows the pre-call notice first and only generates after the exact acknowledgement", async () => {
  mount((command) => command === "feature_summary_notice" ? notice : view);
  fireEvent.click(await screen.findByRole("button", { name: "Generate summary…" }));
  expect(await screen.findByRole("group", { name: "Headless AI call" })).toBeTruthy();
  expect(screen.getByLabelText("Prompt template").textContent).toContain("Summarise");
  expect(calls("feature_summary_generate")).toHaveLength(0);
  fireEvent.click(screen.getByRole("button", { name: "Run with Claude" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("feature_summary_generate",
    { target, ack: { prompt_id: "session.summary", harness: "claude", revision: "rev1" } }));
});

it("cancelling the notice never calls the harness", async () => {
  mount((command) => command === "feature_summary_notice" ? notice : view);
  fireEvent.click(await screen.findByRole("button", { name: "Generate summary…" }));
  fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
  expect(calls("feature_summary_generate")).toHaveLength(0);
  expect(screen.queryByRole("group", { name: "Headless AI call" })).toBeNull();
});

it("reports a refused summary and a finished one", async () => {
  mount((command) => { if (command === "feature_summary_notice") throw { kind: "conflict", message: "The feature is stopped; start it before generating a summary." }; return view; });
  fireEvent.click(await screen.findByRole("button", { name: "Generate summary…" }));
  expect((await screen.findByRole("alert")).textContent).toContain("stopped");
  cleanup();
  mount(() => ({ ...view, summary: "Done", summary_outcome: "Failed to generate summary: model unavailable" }));
  expect(await screen.findByText(/Failed to generate summary/)).toBeTruthy();
});

it("offers a reload when the feature changed elsewhere and keeps the draft's baseline", async () => {
  let current = view;
  mount((command) => command === "feature_actions_load" ? current : view);
  await screen.findByRole("textbox", { name: "Nickname" });
  current = { ...view, nickname: "Elsewhere" };
  await waitFor(() => screen.getByText(/changed elsewhere/), { timeout: 4000 });
  expect((screen.getByRole("textbox", { name: "Nickname" }) as HTMLInputElement).value).toBe("");
  fireEvent.click(screen.getByRole("button", { name: "Reload" }));
  await waitFor(() => expect((screen.getByRole("textbox", { name: "Nickname" }) as HTMLInputElement).value).toBe("Elsewhere"));
}, 8000);
