// @vitest-environment jsdom
import { creationChoices } from "./creationChoices";
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import CreateFeatureForm from "../src/CreateFeatureForm";
import CreateProjectForm from "../src/CreateProjectForm";
import WelcomePanel from "../src/WelcomePanel";
import type { HarnessInfo } from "../src/api";

vi.mock("../src/TerminalPane", () => ({ default: () => null }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const supported: HarnessInfo[] = [{ slug: "claude", display_name: "Claude Code" }, { slug: "codex", display_name: "Codex" }];
const modes = [
  { slug: "vibeless" as const, display_name: "Vibeless", description: "Asks before it edits a file or runs a command." },
  { slug: "vibe" as const, display_name: "Vibe", description: "Edits files without asking; asks before commands." },
];
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.resetAllMocks(); });

function mount(element: React.ReactElement, handle: (command: string, args: unknown) => unknown) {
  vi.mocked(invoke).mockImplementation(async (command, args) => handle(command, args));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  clients.push(client);
  return render(<QueryClientProvider client={client}>{element}</QueryClientProvider>);
}

it("welcome explains the three steps and adds a project", async () => {
  const onAddProject = vi.fn();
  mount(<WelcomePanel onAddProject={onAddProject} onOpenAvailability={vi.fn()} />,
    (command) => command === "allowed_harnesses" ? supported : null);
  const steps = screen.getByRole("list", { name: "Getting started" });
  expect(within(steps).getAllByRole("listitem").map((step) => step.querySelector("strong")?.textContent))
    .toEqual(["Add a project", "Create a feature", "Send a prompt"]);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("allowed_harnesses", { projectId: null }));
  expect(screen.queryByText("No agent is available yet.")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /Add a project/ }));
  expect(onAddProject).toHaveBeenCalled();
});

it("welcome says how to get an agent when none can start, and checks again", async () => {
  let agents: HarnessInfo[] = [];
  const onOpenAvailability = vi.fn();
  mount(<WelcomePanel onAddProject={vi.fn()} onOpenAvailability={onOpenAvailability} />,
    (command) => command === "allowed_harnesses" ? agents : null);
  expect(await screen.findByText("No agent is available yet.")).toBeTruthy();
  expect(within(screen.getByRole("list", { name: "Install an agent" })).getAllByRole("link").map((link) => link.textContent))
    .toEqual(["Claude Code", "Codex", "OpenCode", "Pi"]);
  fireEvent.click(screen.getByRole("button", { name: "Agent availability…" }));
  expect(onOpenAvailability).toHaveBeenCalled();
  agents = supported;
  fireEvent.click(screen.getByRole("button", { name: /Check again/ }));
  await waitFor(() => expect(screen.queryByText("No agent is available yet.")).toBeNull());
});

it("new feature keeps Agent and Mode side by side with the mode explained under Mode and the notice below", async () => {
  const onOpenAvailability = vi.fn();
  mount(<CreateFeatureForm projectId="p" projectName="demo" agents={supported} modes={modes}
    isGit={false} defaultUseWorktree={false} pending={false} onOpenAvailability={onOpenAvailability}
    onCancel={vi.fn()} onSubmit={vi.fn()} />,
    (command) => command === "project_harness_choices" ? creationChoices([])
      : command === "feature_presets_choices" ? [] : null);
  const notice = await screen.findByText("No agent is available yet.");
  const row = screen.getByLabelText("Agent").closest(".form-row")!;
  expect(within(row as HTMLElement).getByLabelText(/^Mode/)).toBeTruthy();
  expect(row.contains(notice)).toBe(false);
  expect(within(row as HTMLElement).getByText("Asks before it edits a file or runs a command.")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Agent availability…" }));
  expect(onOpenAvailability).toHaveBeenCalled();
});

it("add project checks the folder on blur and shows a failed create inline", async () => {
  const onEdit = vi.fn();
  const view = (error: string | null) => <CreateProjectForm agents={supported} pending={false} error={error}
    onEdit={onEdit} onCancel={vi.fn()} onSubmit={vi.fn()} />;
  const { rerender } = mount(view(null), (command, args) => {
    if (command === "allowed_harnesses") return supported;
    if (command === "create_project") {
      const request = (args as { request: { path: string; dry_run: boolean } }).request;
      expect(request.dry_run).toBe(true);
      if (request.path === "/missing") throw { kind: "internal", message: "Path does not exist: /missing" };
      return { ok: true, project_id: null, project_path: "/code/repo", is_git: true, message: "Dry run" };
    }
    return null;
  });
  const path = screen.getByLabelText(/^Repository path/);
  fireEvent.change(path, { target: { value: "/missing" } });
  expect(onEdit).toHaveBeenCalled();
  fireEvent.blur(path);
  expect((await screen.findByRole("alert")).textContent).toBe("Path does not exist: /missing");
  fireEvent.change(path, { target: { value: "/code/repo/src" } });
  fireEvent.blur(path);
  expect(await screen.findByText("Git repository found at /code/repo.")).toBeTruthy();
  expect(screen.getByLabelText(/^Preferred agent/)).toBeTruthy();
  expect(screen.getByText("The agent new features use by default. You can change it per feature.")).toBeTruthy();
  rerender(<QueryClientProvider client={clients[0]}>{view("Project 'repo' already exists")}</QueryClientProvider>);
  expect(screen.getByRole("alert").textContent).toContain("Could not add the project. Project 'repo' already exists");
});
