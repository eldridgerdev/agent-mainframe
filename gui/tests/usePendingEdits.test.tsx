// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import PlanPanel from "../src/PlanPanel";
import { usePendingEdits } from "../src/usePendingEdits";
import type { PlanView } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.forEach((client) => client.clear()); clients.length = 0; vi.clearAllMocks(); });
const brief: PlanView = {
  interview_key: "feature", feature_name: "Feature", kind: "full", phase: "brief", step_key: "brief",
  question_index: 0, question_count: 0, question: null, editor_text: "Saved brief", selected_option: null,
  review_markdown: null, critique: null, attached_docs: [], kickoff_target: null,
};
const entry = { project_id: "project", feature_id: "feature", feature_name: "Feature", count: 1,
  first_id: "edit", first_path: "a.ts" };

it.each(["saved", "discarded"])("opens after a plan draft is %s even though the field stays nonempty", async (result) => {
  const onOpen = vi.fn(); const deferred = vi.fn();
  vi.mocked(invoke).mockResolvedValue([entry]);
  const client = new QueryClient(); clients.push(client);
  function Harness({ view }: { view: PlanView }) {
    usePendingEdits(vi.fn(), vi.fn(), { blocked: false, activeTarget: null, onOpen, onDeferred: deferred });
    return <PlanPanel view={view} precall={null} busy={false} onAct={vi.fn(async () => {})} />;
  }
  const { rerender } = render(<QueryClientProvider client={client}><Harness view={brief} /></QueryClientProvider>);
  const field = screen.getByRole("textbox", { name: "Plan answer" }) as HTMLTextAreaElement;
  fireEvent.change(field, { target: { value: "Revised brief" } });
  await waitFor(() => expect(deferred).toHaveBeenCalledWith("Automatic review waits until you save or discard your draft."));
  expect(onOpen).not.toHaveBeenCalled();
  if (result === "saved") rerender(<QueryClientProvider client={client}><Harness view={{ ...brief, editor_text: "Revised brief" }} /></QueryClientProvider>);
  else fireEvent.change(field, { target: { value: brief.editor_text } });
  await waitFor(() => expect(onOpen).toHaveBeenCalledWith(entry));
  expect(field.value).toBe(result === "saved" ? "Revised brief" : "Saved brief");
});

it("ignores a minimized saved plan dialog while retaining its values", async () => {
  const onOpen = vi.fn(); vi.mocked(invoke).mockResolvedValue([entry]);
  const client = new QueryClient(); clients.push(client);
  function Harness() {
    usePendingEdits(vi.fn(), vi.fn(), { blocked: false, activeTarget: null, onOpen });
    return <div hidden><div role="dialog"><PlanPanel view={brief} precall={null} busy={false} onAct={vi.fn(async () => {})} /></div></div>;
  }
  render(<QueryClientProvider client={client}><Harness /></QueryClientProvider>);
  await waitFor(() => expect(onOpen).toHaveBeenCalledWith(entry));
});
