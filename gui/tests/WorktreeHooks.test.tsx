// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { CreateFeatureForm, TodoNewFeatureForm } from "../src/App";
import type { Project } from "../src/api";

vi.mock("../src/TerminalPane", () => ({ default: () => null }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const agents = [{ slug: "claude" as const, display_name: "Claude" }];
const modes = [{ slug: "vibe" as const, display_name: "Vibe", description: "" }];
const prompt = { title: "Choose stack", options: ["node", "rust & tools"] };
const projects: Project[] = ["one", "two"].map((id) => ({
  id, name: id, repo: `/tmp/${id}`, is_git: true, features: [],
}));
const clients: QueryClient[] = [];

// The forms also read which harnesses and presets they may offer; only the hook prompt
// is under test here, so each test scripts `hookPrompt` alone.
const hookPrompt = vi.fn();
beforeEach(() => {
  hookPrompt.mockResolvedValue(prompt);
  vi.mocked(invoke).mockImplementation(async (command, args) =>
    command === "allowed_harnesses" ? agents
      : command === "feature_presets_choices" ? []
        : hookPrompt(command, args));
});
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.resetAllMocks(); });

function mount(element: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  clients.push(client);
  return render(<QueryClientProvider client={client}>{element}</QueryClientProvider>);
}

it.each(["none", "quick", "full"] as const)("includes a chosen hook for ordinary %s creation", async (kind) => {
  const onSubmit = vi.fn();
  mount(<CreateFeatureForm projectId="one" projectName="one" agents={agents} modes={modes}
    isGit defaultUseWorktree pending={false} onCancel={vi.fn()} onSubmit={onSubmit} />);
  fireEvent.change(screen.getByLabelText("Branch / feature name"), { target: { value: "new-work" } });
  const select = await screen.findByLabelText("Choose stack");
  if (kind !== "none") fireEvent.click(screen.getByRole("radio", { name: kind === "quick" ? "Quick Plan" : "Full Plan" }));
  const button = screen.getByRole("button", { name: kind === "none" ? "Create feature" : "Create and plan" });
  expect((button as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(select, { target: { value: "rust & tools" } });
  fireEvent.click(button);
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({
    branch: "new-work", use_worktree: true, hook_choice: "rust & tools", plan_mode: kind !== "none",
  }), kind);
});

it.each(["plan", "launch"] as const)("includes the choice for a TODO %s", async (kind) => {
  const onSubmit = vi.fn();
  mount(<TodoNewFeatureForm kind={kind} todoTitle="Improve API" projects={[projects[0]]} agents={agents} modes={modes}
    pending={false} onCancel={vi.fn()} onSubmit={onSubmit} />);
  const select = await screen.findByLabelText("Choose stack");
  fireEvent.change(select, { target: { value: "rust & tools" } });
  fireEvent.click(screen.getByRole("button", { name: kind === "plan" ? "Start plan" : "Create and start" }));
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ hook_choice: "rust & tools", project_name: "one" }), kind === "plan" ? "full" : "none");
});

it("requires a new choice when the TODO destination project changes", async () => {
  const onSubmit = vi.fn();
  mount(<TodoNewFeatureForm kind="launch" todoTitle="Improve API" projects={projects} agents={agents} modes={modes}
    pending={false} onCancel={vi.fn()} onSubmit={onSubmit} />);
  fireEvent.change(await screen.findByLabelText("Choose stack"), { target: { value: "rust & tools" } });
  fireEvent.change(screen.getByLabelText("Project"), { target: { value: "two" } });
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("worktree_hook_prompt", { projectId: "two" }));
  expect((await screen.findByLabelText("Choose stack") as HTMLSelectElement).value).toBe("");
  fireEvent.submit(screen.getByRole("dialog").querySelector("form")!);
  expect(onSubmit).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText("Choose stack"), { target: { value: "node" } });
  await waitFor(() => expect((screen.getByRole("button", { name: "Create and start" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Create and start" }));
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ project_name: "two", hook_choice: "node" }), "none");
});

it("blocks a failed prompt lookup until retry succeeds", async () => {
  hookPrompt.mockReset().mockRejectedValueOnce({ kind: "internal", message: "Unavailable" }).mockResolvedValue(null);
  const onSubmit = vi.fn();
  mount(<TodoNewFeatureForm kind="launch" todoTitle="Improve API" projects={[projects[0]]} agents={agents} modes={modes}
    pending={false} onCancel={vi.fn()} onSubmit={onSubmit} />);
  await screen.findByRole("alert");
  fireEvent.submit(screen.getByRole("dialog").querySelector("form")!);
  expect(onSubmit).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  const button = screen.getByRole("button", { name: "Create and start" });
  await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(button);
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ hook_choice: null }), "none");
});

it("cancel applies neither the selected hook nor feature creation", async () => {
  const onSubmit = vi.fn();
  const onCancel = vi.fn();
  mount(<TodoNewFeatureForm kind="plan" todoTitle="Improve API" projects={[projects[0]]} agents={agents} modes={modes}
    pending={false} onCancel={onCancel} onSubmit={onSubmit} />);
  fireEvent.change(await screen.findByLabelText("Choose stack"), { target: { value: "node" } });
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(onCancel).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
  expect(onCancel).toHaveBeenCalledOnce();
  expect(onSubmit).not.toHaveBeenCalled();
  expect(vi.mocked(invoke).mock.calls.every(([command]) =>
    command === "worktree_hook_prompt" || command === "allowed_harnesses" || command === "feature_presets_choices")).toBe(true);
});

it("does not request or send a worktree hook for the repository checkout", () => {
  const onSubmit = vi.fn();
  mount(<CreateFeatureForm projectId="one" projectName="one" agents={agents} modes={modes}
    isGit defaultUseWorktree={false} pending={false} onCancel={vi.fn()} onSubmit={onSubmit} />);
  fireEvent.change(screen.getByLabelText("Branch / feature name"), { target: { value: "new-work" } });
  fireEvent.click(screen.getByRole("button", { name: "Create feature" }));
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ use_worktree: false, hook_choice: null }), "none");
  expect(hookPrompt).not.toHaveBeenCalled();
});
