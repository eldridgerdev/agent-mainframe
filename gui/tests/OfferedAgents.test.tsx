// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { CreateFeatureForm, TodoNewFeatureForm } from "../src/App";
import type { HarnessInfo, Project } from "../src/api";

vi.mock("../src/TerminalPane", () => ({ default: () => null }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const harness = (slug: HarnessInfo["slug"], display_name: string): HarnessInfo => ({ slug, display_name });
const supported = [harness("claude", "Claude"), harness("opencode", "OpenCode"), harness("codex", "Codex"), harness("pi", "Pi")];
const modes = [{ slug: "vibe" as const, display_name: "Vibe", description: "" }];
const projects: Project[] = ["one", "two"].map((id) => ({
  id, name: id, repo: `/tmp/${id}`, is_git: true, features: [],
}));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.resetAllMocks(); });

/** `allowed` answers per project id; anything else has no worktree hook. */
function mount(element: React.ReactElement, allowed: (projectId: string | null) => HarnessInfo[]) {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "allowed_harnesses") return allowed((args as { projectId: string | null }).projectId);
    return command === "feature_presets_choices" ? [] : null;
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  clients.push(client);
  return render(<QueryClientProvider client={client}>{element}</QueryClientProvider>);
}
const offered = () => Array.from((screen.getByLabelText("Harness") as HTMLSelectElement).options).map((option) => option.text);

it("offers a new feature only the harnesses its project allows and submits one of them", async () => {
  const onSubmit = vi.fn();
  mount(<CreateFeatureForm projectId="one" projectName="one" agents={supported} modes={modes}
    isGit={false} defaultUseWorktree={false} pending={false} onCancel={vi.fn()} onSubmit={onSubmit} />,
    () => [supported[2], supported[3]]);
  await waitFor(() => expect(offered()).toEqual(["Codex", "Pi"]));
  expect(invoke).toHaveBeenCalledWith("allowed_harnesses", { projectId: "one" });
  // The form opened on Claude, the first supported harness; it is not sent.
  fireEvent.change(screen.getByLabelText("Branch / feature name"), { target: { value: "new-work" } });
  fireEvent.click(screen.getByRole("button", { name: "Create feature" }));
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ agent: "codex" }), "none");
});

it("follows the chosen project's own list when starting a TODO in a new feature", async () => {
  const onSubmit = vi.fn();
  mount(<TodoNewFeatureForm kind="launch" todoTitle="Improve API" projects={projects} agents={supported} modes={modes}
    pending={false} onCancel={vi.fn()} onSubmit={onSubmit} />,
    (projectId) => projectId === "one" ? [supported[0], supported[2]] : [supported[3]]);
  await waitFor(() => expect(offered()).toEqual(["Claude", "Codex"]));
  fireEvent.change(screen.getByLabelText("Harness"), { target: { value: "codex" } });
  fireEvent.change(screen.getByLabelText("Project"), { target: { value: "two" } });
  await waitFor(() => expect(offered()).toEqual(["Pi"]));
  const button = screen.getByRole("button", { name: "Create and start" }) as HTMLButtonElement;
  await waitFor(() => expect(button.disabled).toBe(false));
  fireEvent.click(button);
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ project_name: "two", agent: "pi" }), "none");
});

it("keeps every supported harness on offer when the allowed list cannot be read", async () => {
  mount(<CreateFeatureForm projectId="one" projectName="one" agents={supported} modes={modes}
    isGit={false} defaultUseWorktree={false} pending={false} onCancel={vi.fn()} onSubmit={vi.fn()} />,
    () => { throw { kind: "internal", message: "Invalid JSON" }; });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("allowed_harnesses", { projectId: "one" }));
  expect(offered()).toEqual(["Claude", "OpenCode", "Codex", "Pi"]);
});
