// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import PromptOverridesPanel from "../src/PromptOverridesPanel";
import type { OverrideRow, OverridesView, SaveOverride } from "../src/promptOverridesApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.clearAllMocks(); vi.useRealTimers(); });

const context = { kind: "feature" as const, project_id: "project", feature_id: "feature" };
const projects = [{ id: "project", name: "Project", repo: "/repo", is_git: true, features: [{
  id: "feature", name: "Feature", branch: "feature", workdir: "/worktree", is_worktree: true,
  status: "idle" as const, agent: "codex" as const, mode: "vibeless" as const, sessions: [],
}] }];
const walkthrough: OverrideRow = {
  id: "review.walkthrough", title: "Final Review: file walkthrough", summary: "Explains one file.",
  placeholders: ["file_path", "patch"], source: "built_in", source_harness: null,
  effective_template: "Explain {{file_path}}\n{{patch}}", default_template: "Explain {{file_path}}\n{{patch}}",
  stored: [], revision: "r1",
};
const summary: OverrideRow = {
  id: "session.summary", title: "Session summary", summary: "One line.", placeholders: ["recent_lines"],
  source: "global", source_harness: "codex", effective_template: "GLOBAL codex {{recent_lines}}",
  default_template: "Summarize {{recent_lines}}",
  stored: [
    { scope: "project", harness: null, template: "PROJECT {{recent_lines}}" },
    { scope: "global", harness: "codex", template: "GLOBAL codex {{recent_lines}}" },
  ], revision: "s1",
};
const view = (rows: OverrideRow[] = [walkthrough, summary], extra: Partial<OverridesView> = {}): OverridesView => ({
  context, context_label: "Project / Feature", repo: "/repo", workdir: "/worktree", harness: "codex",
  scopes: [
    { scope: "feature", label: "This feature", available: true, reason: null },
    { scope: "project", label: "This project (amf.json)", available: true, reason: null },
    { scope: "global", label: "Global (all projects)", available: true, reason: null },
  ], project_config_error: null, rows, ...extra,
});

function mount(loads: () => OverridesView, save?: (request: SaveOverride) => Promise<OverridesView>, initialPromptId: string | null = null) {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "prompt_overrides_load") return loads();
    if (command === "prompt_overrides_save" && save) return save((args as { request: SaveOverride }).request);
    if (command === "prompt_overrides_clear") return view([walkthrough, { ...summary, stored: [summary.stored[1]], revision: "s2" }]);
    throw new Error(`Unexpected command ${command}`);
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  const onClose = vi.fn();
  // Stands in for Final Review's own Escape handler underneath the manager.
  const outerEscape = vi.fn();
  window.addEventListener("keydown", outerEscape);
  render(<QueryClientProvider client={client}><PromptOverridesPanel initialContext={context}
    initialPromptId={initialPromptId} initialHarness={null} fromPrecall={false} projects={projects} onClose={onClose} /></QueryClientProvider>);
  return { onClose, outerEscape, cleanupOuter: () => window.removeEventListener("keydown", outerEscape) };
}
const calls = (name: string) => vi.mocked(invoke).mock.calls.filter(([command]) => command === name);
const template = () => screen.getByRole("textbox", { name: /Template/ }) as HTMLTextAreaElement;

it("lists every prompt with its source and shows the effective template, placeholders and stored layers", async () => {
  mount(() => view(), undefined, "session.summary");
  expect(await screen.findByRole("button", { name: /Final Review: file walkthrough.*Built-in/ })).toBeTruthy();
  expect(screen.getByRole("button", { name: /Session summary.*Global · Codex/ }).getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByLabelText("Effective template").textContent).toBe("GLOBAL codex {{recent_lines}}");
  expect(screen.getByText("{{recent_lines}}")).toBeTruthy();
  fireEvent.click(screen.getByText("Built-in default"));
  expect(screen.getByLabelText("Built-in default").textContent).toBe("Summarize {{recent_lines}}");
  const slots = screen.getAllByRole("listitem").map((item) => item.textContent);
  expect(slots[0]).toContain("Project (amf.json) · all harnesses");
  expect(slots[1]).toContain("Global · Codex");
  expect(slots[1]).toContain("in effect");
  expect(calls("prompt_overrides_load")[0][1]).toEqual({ context, harness: null });
});

it("saves a new override at the chosen scope and harness with the revision it was based on", async () => {
  const save = vi.fn(async () => view([{ ...walkthrough, source: "project", source_harness: "pi", revision: "r2",
    stored: [{ scope: "project", harness: "pi", template: "Mine" }] }, summary]));
  mount(() => view(), save);
  fireEvent.click(await screen.findByRole("button", { name: "New override…" }));
  expect(template().value).toBe(walkthrough.effective_template);
  fireEvent.change(screen.getByRole("combobox", { name: "Save to scope" }), { target: { value: "project" } });
  fireEvent.change(screen.getByRole("combobox", { name: "Harness" }), { target: { value: "pi" } });
  fireEvent.change(template(), { target: { value: "Mine" } });
  const button = screen.getByRole("button", { name: "Save override" });
  fireEvent.click(button); fireEvent.click(button);
  await waitFor(() => expect(screen.getByRole("status").textContent).toContain("Saved the Project (amf.json) · Pi override"));
  expect(save).toHaveBeenCalledTimes(1);
  expect(save).toHaveBeenCalledWith({ context, prompt_id: "review.walkthrough", scope: "project", harness: "pi",
    template: "Mine", revision: "r1", view_harness: null });
  expect(screen.queryByRole("textbox", { name: /Template/ })).toBeNull();
  expect(screen.getByLabelText("Effective template").textContent).toBe(walkthrough.effective_template);
});

it("requires a reload after an external change and keeps the draft through a refused save", async () => {
  let current = view();
  const save = vi.fn(async (request: SaveOverride) => {
    if (request.revision !== current.rows[0].revision) throw { kind: "conflict", message: "This prompt's overrides changed outside this window. Reload." };
    return current;
  });
  mount(() => current, save);
  fireEvent.click(await screen.findByRole("button", { name: "New override…" }));
  fireEvent.change(template(), { target: { value: "draft text" } });

  // The TUI saves the same prompt; the backend refuses even before a poll notices.
  current = view([{ ...walkthrough, revision: "r9", source: "global", stored: [{ scope: "global", harness: null, template: "TUI" }] }, summary]);
  fireEvent.click(screen.getByRole("button", { name: "Save override" }));
  expect((await screen.findByRole("alert")).textContent).toContain("changed outside this window");
  expect(template().value).toBe("draft text");
  await waitFor(() => expect(screen.getByRole("button", { name: "Reload current version" })).toBeTruthy());
  expect((screen.getByRole("button", { name: "Save override" }) as HTMLButtonElement).disabled).toBe(true);

  fireEvent.click(screen.getByRole("button", { name: "Reload current version" }));
  expect(template().value).toBe("draft text");
  expect(screen.queryByRole("button", { name: "Reload current version" })).toBeNull();
  expect((screen.getByRole("button", { name: "Save override" }) as HTMLButtonElement).disabled).toBe(false);
  expect(screen.queryByText(/This replaces the existing/)).toBeNull();
  fireEvent.change(screen.getByRole("combobox", { name: "Save to scope" }), { target: { value: "global" } });
  expect(screen.getByText(/This replaces the existing Global · all harnesses override/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Save override" }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  expect(save.mock.calls[1][0]).toMatchObject({ revision: "r9", template: "draft text", scope: "global" });
});

it("protects unsaved edits on Escape, prompt switches and context changes, and owns the Escape key", async () => {
  const { onClose, outerEscape, cleanupOuter } = mount(() => view());
  fireEvent.click(await screen.findByRole("button", { name: "New override…" }));
  // An unchanged draft closes freely; a changed one asks first.
  fireEvent.change(template(), { target: { value: "changed" } });
  fireEvent.keyDown(document.body, { key: "Escape" });
  expect(outerEscape).not.toHaveBeenCalled();
  expect(onClose).not.toHaveBeenCalled();
  const confirm = screen.getByRole("alertdialog", { name: "Discard unsaved template" });
  fireEvent.click(within(confirm).getByRole("button", { name: "Keep editing" }));
  expect(template().value).toBe("changed");

  fireEvent.click(screen.getByRole("button", { name: /Session summary/ }));
  expect(screen.getByRole("alertdialog", { name: "Discard unsaved template" })).toBeTruthy();
  expect(template().value).toBe("changed");
  fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
  expect(screen.getByLabelText("Effective template").textContent).toBe("GLOBAL codex {{recent_lines}}");

  fireEvent.click(screen.getByRole("button", { name: "Edit Project (amf.json) · all harnesses" }));
  expect(template().value).toBe("PROJECT {{recent_lines}}");
  fireEvent.change(template(), { target: { value: "edited" } });
  fireEvent.change(screen.getByRole("combobox", { name: "Override context" }), { target: { value: JSON.stringify({ kind: "global" }) } });
  expect(screen.getByText(/Discard your unsaved template changes and switch context/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
  await waitFor(() => expect(calls("prompt_overrides_load").some(([, args]) => (args as { context: { kind: string } }).context.kind === "global")).toBe(true));

  fireEvent.keyDown(document.body, { key: "Escape" });
  expect(onClose).toHaveBeenCalledTimes(1);
  expect(outerEscape).not.toHaveBeenCalled();
  cleanupOuter();
});

it("clears one stored override only after explicit confirmation", async () => {
  mount(() => view(), undefined, "session.summary");
  fireEvent.click(await screen.findByRole("button", { name: "Clear Global · Codex" }));
  const confirm = screen.getByRole("alertdialog", { name: "Confirm clear override" });
  expect(confirm.textContent).toContain("Clear the Global · Codex override for Session summary?");
  fireEvent.click(within(confirm).getByRole("button", { name: "Keep it" }));
  expect(calls("prompt_overrides_clear")).toHaveLength(0);

  fireEvent.click(screen.getByRole("button", { name: "Clear Global · Codex" }));
  fireEvent.click(screen.getByRole("button", { name: "Clear override" }));
  await waitFor(() => expect(calls("prompt_overrides_clear")).toHaveLength(1));
  expect(calls("prompt_overrides_clear")[0][1]).toEqual({ request: {
    context, prompt_id: "session.summary", scope: "global", harness: "codex", revision: "s1", view_harness: null,
  } });
  expect(await screen.findByText(/Cleared the Global · Codex override/)).toBeTruthy();
});

it("clears against the revision the user confirmed, not a newer polled one", async () => {
  let current = view();
  mount(() => current, undefined, "session.summary");
  fireEvent.click(await screen.findByRole("button", { name: "Clear Global · Codex" }));
  const confirm = screen.getByRole("alertdialog", { name: "Confirm clear override" });
  expect(within(confirm).getByLabelText("Template to clear").textContent).toBe("GLOBAL codex {{recent_lines}}");

  // The TUI rewrites that slot while the confirmation is open; the next poll sees it.
  current = view([walkthrough, { ...summary, revision: "s9", effective_template: "TUI {{recent_lines}}",
    stored: [summary.stored[0], { scope: "global", harness: "codex", template: "TUI {{recent_lines}}" }] }]);
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  expect((await within(confirm).findByRole("alert")).textContent).toContain("changed after you chose Clear");
  expect(within(confirm).getByLabelText("Template to clear").textContent).toBe("GLOBAL codex {{recent_lines}}");
  const clear = within(confirm).getByRole("button", { name: "Clear override" }) as HTMLButtonElement;
  expect(clear.disabled).toBe(true);
  fireEvent.click(clear);
  expect(calls("prompt_overrides_clear")).toHaveLength(0);
});

it("reports a broken amf.json and disables unavailable scopes with their reason", async () => {
  mount(() => view([walkthrough], {
    project_config_error: "/repo/amf.json is not valid JSON (EOF); fix it first",
    scopes: [
      { scope: "feature", label: "This feature", available: false, reason: "No AMF database is open" },
      { scope: "project", label: "This project (amf.json)", available: false, reason: "/repo/amf.json is not valid JSON (EOF); fix it first" },
      { scope: "global", label: "Global (all projects)", available: true, reason: null },
    ],
  }));
  expect((await screen.findByRole("alert")).textContent).toContain("Project overrides are ignored.");
  fireEvent.click(screen.getByRole("button", { name: "New override…" }));
  const scope = screen.getByRole("combobox", { name: "Save to scope" }) as HTMLSelectElement;
  expect(scope.value).toBe("global");
  const options = Array.from(scope.options);
  expect(options.find((option) => option.value === "project")?.disabled).toBe(true);
  expect(options.find((option) => option.value === "feature")?.textContent).toContain("No AMF database is open");
  fireEvent.change(template(), { target: { value: "   " } });
  expect((screen.getByRole("button", { name: "Save override" }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => undefined);
});
