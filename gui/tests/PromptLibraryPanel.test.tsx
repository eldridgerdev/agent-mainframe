// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import PromptLibraryPanel from "../src/PromptLibraryPanel";
import type { LibraryEntry, LibraryView, ResolvePrompt } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.clearAllMocks(); });
const target = { project_id: "project", feature_id: "feature", session_id: "agent" };
const scope = { kind: "feature" as const, project_id: "project", feature_id: "feature" };
const entry: LibraryEntry = {
  key: "template", name: "Fix a bug", description: "Reusable repair", source: "User", tags: ["bug"],
  body: "Fix {{area}} in {{env: dev|prod}} with {{notes}} and {{rust|go}}.", slots: [
    { key: "area", label: "Area", kind: "text", default: "auth", initial_value: "auth", required: true },
    { key: "env", label: "env", kind: "select", options: ["dev", "prod"], initial_value: "dev", required: false },
    { key: "notes", label: "Notes", kind: "multi_line", default: "Keep tests", initial_value: "Keep tests", required: false },
    { key: "rust|go", label: "Choose an option", kind: "select", options: ["rust", "go"], initial_value: "rust", required: false },
  ],
};
const view: LibraryView = { entries: [entry, { ...entry, key: "project-template", source: "Project", name: "Project prompt", slots: [], body: "Plain prompt" }],
  available_keys: ["template", "project-template"], targets: [
  { target, label: "Project / Feature / Claude", stopped: false },
  { target: { ...target, session_id: "codex" }, label: "Project / Feature / Codex", stopped: true },
] };
const projects = [{ id: "project", name: "Project", repo: "/repo", is_git: true, features: [{
  id: "feature", name: "Feature", branch: "feature", workdir: "/worktree", is_worktree: true,
  status: "idle" as const, agent: "claude" as const, mode: "vibeless" as const, sessions: [],
}] }];
function mock() {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "prompt_library_load") return view;
    if (command === "prompt_library_resolve") return (args as { request: ResolvePrompt }).request.entry_key === "project-template" ? "Plain prompt" : "Resolved repair";
    throw new Error(`Unexpected command ${command}`);
  });
}
function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  const onClose = vi.fn(); const onInsert = vi.fn();
  render(<QueryClientProvider client={client}><PromptLibraryPanel initialScope={scope} initialTarget={target}
    projects={projects} onClose={onClose} onInsert={onInsert} /></QueryClientProvider>);
  return { client, onClose, onInsert };
}
const previewText = () => screen.getByLabelText("Resolved prompt").textContent;
const resolveCalls = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "prompt_library_resolve");
async function choose() {
  fireEvent.click(await screen.findByRole("button", { name: /Fix a bug/ }));
  expect(previewText()).toBe("Fix auth in dev with Keep tests and rust.");
  await waitFor(() => expect((screen.getByRole("button", { name: "Add to draft" }) as HTMLButtonElement).disabled).toBe(false));
}
const insertCalls = () => vi.mocked(invoke).mock.calls.filter(([command, args]) => command === "prompt_library_resolve" && (args as { request: ResolvePrompt }).request.target !== null);

it("previews defaults, fills text, multiline and choices, then inserts only after an explicit action", async () => {
  mock(); const { onInsert } = mount(); await choose();
  expect((screen.getByRole("textbox", { name: "Area (required)" }) as HTMLInputElement).value).toBe("auth");
  expect((screen.getByRole("textbox", { name: "Notes" }) as HTMLTextAreaElement).value).toBe("Keep tests");
  expect(screen.getAllByText("#bug")).toHaveLength(2);
  fireEvent.click(screen.getByText("Original template"));
  expect(screen.getByText(entry.body)).toBeTruthy();
  fireEvent.change(screen.getByRole("textbox", { name: "Area (required)" }), { target: { value: "login 世界" } });
  fireEvent.change(screen.getByRole("textbox", { name: "Notes" }), { target: { value: "one\ntwo" } });
  fireEvent.change(screen.getByRole("combobox", { name: "env" }), { target: { value: "prod" } });
  fireEvent.change(screen.getByRole("combobox", { name: "Choose an option" }), { target: { value: "go" } });
  expect(previewText()).toBe("Fix login 世界 in prod with one\ntwo and go.");
  // The preview renders locally; typing never round-trips to the backend.
  expect(resolveCalls()).toHaveLength(0);
  expect(onInsert).not.toHaveBeenCalled();
  await waitFor(() => expect((screen.getByRole("button", { name: "Add to draft" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Add to draft" }));
  await waitFor(() => expect(onInsert).toHaveBeenCalledWith(target, "Resolved repair"));
  expect(insertCalls()).toHaveLength(1); expect(resolveCalls()).toHaveLength(1);
  expect(insertCalls()[0][1]).toEqual({ request: {
    scope, entry_key: "template", values: [["area", "login 世界"], ["env", "prod"], ["notes", "one\ntwo"], ["rust|go", "go"]], target,
  } });
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "terminal_submit_prompt" || command === "terminal_input")).toBe(false);
});

it("blocks empty required fields and allows selecting an explicit stopped destination", async () => {
  mock(); const { onInsert } = mount(); await choose();
  fireEvent.change(screen.getByRole("textbox", { name: "Area (required)" }), { target: { value: "  " } });
  expect((screen.getByRole("button", { name: "Add to draft" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(screen.getByRole("textbox", { name: "Area (required)" }), { target: { value: "auth" } });
  const stopped = view.targets[1];
  fireEvent.change(screen.getByRole("combobox", { name: "Agent session" }), {
    target: { value: JSON.stringify(["project", "feature", "codex"]) },
  });
  expect(screen.getByText(/Start the session when/)).toBeTruthy();
  await waitFor(() => expect((screen.getByRole("button", { name: "Add to draft" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Add to draft" }));
  await waitFor(() => expect(onInsert).toHaveBeenCalledWith(stopped.target, "Resolved repair"));
});

it("uses shared search requests, source badges and scope switching without retaining the old selection", async () => {
  mock(); const { onInsert } = mount(); await choose();
  fireEvent.change(screen.getByRole("textbox", { name: "Search prompts" }), { target: { value: "#bug" } });
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("prompt_library_load", { scope, query: "#bug" }));
  expect((screen.getByRole("textbox", { name: "Area (required)" }) as HTMLInputElement).value).toBe("auth");
  fireEvent.change(screen.getByRole("combobox", { name: "Library scope" }), { target: { value: JSON.stringify({ kind: "global" }) } });
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("prompt_library_load", { scope: { kind: "global" }, query: "#bug" }));
  expect(screen.getByText("Select a prompt")).toBeTruthy();
  expect(onInsert).not.toHaveBeenCalled();
});

it("locks duplicate insertion synchronously and retains fields through failure for retry", async () => {
  mock(); const { onInsert } = mount(); await choose();
  const original = vi.mocked(invoke).getMockImplementation()!;
  let reject!: (error: unknown) => void;
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "prompt_library_resolve" && (args as { request: ResolvePrompt }).request.target)
      return new Promise((_, no) => { reject = no; });
    return original(command, args, options);
  });
  const button = screen.getByRole("button", { name: "Add to draft" });
  fireEvent.click(button); fireEvent.click(button);
  expect(insertCalls()).toHaveLength(1);
  await act(async () => reject({ kind: "conflict", message: "This template changed. Select it again." }));
  expect(await screen.findByText("This template changed. Select it again.")).toBeTruthy();
  expect((screen.getByRole("textbox", { name: "Area (required)" }) as HTMLInputElement).value).toBe("auth");
  expect(onInsert).not.toHaveBeenCalled();
  vi.mocked(invoke).mockImplementation(original);
  fireEvent.click(screen.getByRole("button", { name: /Project prompt/ }));
  await waitFor(() => expect(previewText()).toBe("Plain prompt"));
  await waitFor(() => expect((screen.getByRole("button", { name: "Add to draft" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Add to draft" }));
  await waitFor(() => expect(onInsert).toHaveBeenCalledOnce());
});

it("drops a deleted destination after refresh without replacing it with another session", async () => {
  mock(); const { client, onInsert } = mount(); await choose();
  const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((command, args, options) => command === "prompt_library_load"
    ? Promise.resolve({ ...view, targets: [view.targets[1]] }) : original(command, args, options));
  await act(async () => { await client.invalidateQueries({ queryKey: ["prompt-library"] }); });
  expect(await screen.findByRole("option", { name: "Selected session no longer available" })).toBeTruthy();
  expect((screen.getByRole("button", { name: "Add to draft" }) as HTMLButtonElement).disabled).toBe(true);
  expect(onInsert).not.toHaveBeenCalled();
});

it("cancels while filling and ignores a late insertion response after unmount", async () => {
  mock(); const { onClose, onInsert } = mount(); await choose();
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(onClose).toHaveBeenCalledOnce(); expect(onInsert).not.toHaveBeenCalled();
  const original = vi.mocked(invoke).getMockImplementation()!;
  let finish!: (text: string) => void;
  vi.mocked(invoke).mockImplementation((command, args, options) => {
    if (command === "prompt_library_resolve" && (args as { request: ResolvePrompt }).request.target)
      return new Promise((resolve) => { finish = resolve; });
    return original(command, args, options);
  });
  fireEvent.click(screen.getByRole("button", { name: "Add to draft" }));
  cleanup();
  await act(async () => finish("Late draft"));
  expect(onInsert).not.toHaveBeenCalled();
});

it("shows empty, unavailable-target and loading failure states with a working retry", async () => {
  vi.mocked(invoke).mockRejectedValue({ kind: "internal", message: "Could not read library" });
  mount(); await screen.findByText("Could not read library");
  vi.mocked(invoke).mockResolvedValue({ entries: [], available_keys: [], targets: [] });
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await screen.findByText("No saved prompts");
  vi.mocked(invoke).mockImplementation(async (command) => command === "prompt_library_load" ? { entries: [entry], available_keys: ["template"], targets: [] } : "Resolved repair");
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  fireEvent.click(await screen.findByRole("button", { name: /Fix a bug/ }));
  await screen.findByText(/Add an allowed agent session/);
  expect((screen.getByRole("button", { name: "Add to draft" }) as HTMLButtonElement).disabled).toBe(true);
});

it("keeps a newer scope visible when an old request resolves late", async () => {
  let finish!: (view: LibraryView) => void;
  vi.mocked(invoke).mockImplementation((command, args) => {
    if (command === "prompt_library_load" && (args as { scope: { kind: string } }).scope.kind === "feature")
      return new Promise((resolve) => { finish = resolve; });
    return Promise.resolve({ entries: [{ ...entry, name: "Global template", source: "Global" }], available_keys: ["template"], targets: [] });
  });
  mount();
  await waitFor(() => expect(finish).toBeTruthy());
  fireEvent.change(screen.getByRole("combobox", { name: "Library scope" }), { target: { value: JSON.stringify({ kind: "global" }) } });
  await screen.findByRole("button", { name: /Global template/ });
  await act(async () => finish(view));
  expect(screen.queryByRole("button", { name: /Fix a bug/ })).toBeNull();
});

it("keeps a selection the search only hides, and clears one changed outside the window", async () => {
  mock(); const { client, onInsert } = mount(); await choose();
  fireEvent.change(screen.getByRole("textbox", { name: "Area (required)" }), { target: { value: "login" } });
  const original = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((command, args, options) => command === "prompt_library_load"
    ? Promise.resolve({ ...view, entries: [view.entries[1]] }) : original(command, args, options));
  await act(async () => { await client.invalidateQueries({ queryKey: ["prompt-library"] }); });
  await waitFor(() => expect(screen.queryByRole("button", { name: /Fix a bug/ })).toBeNull());
  expect((screen.getByRole("textbox", { name: "Area (required)" }) as HTMLInputElement).value).toBe("login");

  const edited = { ...entry, key: "template-v2", body: "Edited {{area}}" };
  vi.mocked(invoke).mockImplementation((command, args, options) => command === "prompt_library_load"
    ? Promise.resolve({ ...view, entries: [edited, view.entries[1]], available_keys: ["template-v2", "project-template"] })
    : original(command, args, options));
  await act(async () => { await client.invalidateQueries({ queryKey: ["prompt-library"] }); });
  expect(await screen.findByText(/“Fix a bug” was changed or removed outside this window/)).toBeTruthy();
  expect(screen.getByText("Select a prompt")).toBeTruthy();
  expect((screen.getByRole("button", { name: "Add to draft" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: /Fix a bug/ }));
  expect(previewText()).toBe("Edited auth");
  expect(screen.getByRole("button", { name: /Fix a bug/ }).getAttribute("aria-pressed")).toBe("true");
  expect(screen.queryByText(/was changed or removed/)).toBeNull();
  expect(onInsert).not.toHaveBeenCalled();
});
