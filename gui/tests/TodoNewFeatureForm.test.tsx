// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import type { FeaturePreset, Project } from "../src/api";
import { TodoNewFeatureForm } from "../src/App";

vi.mock("../src/TerminalPane", () => ({ default: () => null }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async (command) => command === "allowed_harnesses" ? [{ slug: "claude", display_name: "Claude" }] : command === "feature_presets_choices" ? [] : null) }));

afterEach(cleanup);

const project = {
  id: "project-1",
  name: "demo",
  repo: "/tmp/demo",
  is_git: true,
  features: [],
};

describe("TodoNewFeatureForm", () => {
  it("submits a direct TODO launch as a worktree feature without planning", async () => {
    const onSubmit = vi.fn();
    render(
      <QueryClientProvider client={new QueryClient()}><TodoNewFeatureForm
        kind="launch"
        todoTitle="Improve the API"
        projects={[project] as Parameters<typeof TodoNewFeatureForm>[0]["projects"]}
        agents={[{ slug: "claude", display_name: "Claude" }]}
        modes={[{ slug: "vibe", display_name: "Vibe" }]}
        pending={false}
        onCancel={vi.fn()}
        onSubmit={onSubmit}
      /></QueryClientProvider>,
    );

    const button = screen.getByRole("button", { name: "Create and start" });
    await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(button);
    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({
      project_name: "demo",
      branch: "improve-the-api",
      plan_mode: false,
      use_worktree: true,
      dry_run: false,
    }), "none");
  });
});

const agents = [{ slug: "claude" as const, display_name: "Claude" }, { slug: "codex" as const, display_name: "Codex" }];
const modes = [{ slug: "vibe" as const, display_name: "Vibe" }, { slug: "supervibe" as const, display_name: "SuperVibe" },
  { slug: "vibeless" as const, display_name: "Vibeless", unsupported_agents: ["codex" as const] }];
const projects: Project[] = ["demo", "second"].map((id) => ({ id, name: id, repo: `/tmp/${id}`, is_git: true, features: [] }));
const preset: FeaturePreset = { name: "Ship", branch_prefix: "feat", agent: "codex", mode: "supervibe",
  review: true, plan_mode: true, quick_plan: true, enable_chrome: true, remote_control: false };
const clients: QueryClient[] = [];
afterEach(() => { clients.splice(0).forEach((client) => client.clear()); vi.clearAllMocks(); });
const change = (label: string, value: string) => fireEvent.change(screen.getByLabelText(label), { target: { value } });
const click = (name: string) => fireEvent.click(screen.getByRole("button", { name }));
function mount(overrides: Partial<Parameters<typeof TodoNewFeatureForm>[0]> = {}, presets: FeaturePreset[] = [preset]) {
  vi.mocked(invoke).mockImplementation(async (command) => command === "allowed_harnesses" ? agents : command === "feature_presets_choices" ? presets : null);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } }); clients.push(client);
  const props = { kind: "launch" as const, todoTitle: "Improve the API", projects: [projects[0]], agents, modes,
    pending: false, onCancel: vi.fn(), onSubmit: vi.fn(), ...overrides };
  const result = render(<QueryClientProvider client={client}><TodoNewFeatureForm {...props} /></QueryClientProvider>);
  return { ...props, client, rerender: (updated: Partial<typeof props>) => result.rerender(<QueryClientProvider client={client}><TodoNewFeatureForm {...props} {...updated} /></QueryClientProvider>) };
}
async function ready(name = "Create and start") {
  await waitFor(() => expect(screen.getByRole("button", { name })).toHaveProperty("disabled", false));
}
it.each(["none", "full", "quick"] as const)("honors a preset's %s route and subsequent editable choices", async (kind) => {
  const { onSubmit } = mount({}, [{ ...preset, plan_mode: kind !== "none", quick_plan: kind === "quick" }]);
  await screen.findByRole("option", { name: "Ship" }); change("Feature preset", "0");
  expect(screen.getByLabelText("Branch")).toHaveProperty("value", "feat/improve-the-api");
  expect(screen.getByLabelText("Harness")).toHaveProperty("value", "codex");
  change("Branch", "edited/meaningful-name"); fireEvent.click(screen.getByRole("checkbox", { name: "Chrome" }));
  const button = kind === "none" ? "Create and start" : kind === "full" ? "Start plan" : "Start Quick Plan";
  await ready(button); click(button);
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ branch: "edited/meaningful-name", agent: "codex", mode: "supervibe", review: true, enable_chrome: false, plan_mode: kind !== "none" }), kind);
});
it("uses Full Plan initially for Plan TODO and allows intentional direct opt-out", async () => {
  const { onSubmit } = mount({ kind: "plan" }); await ready("Start plan");
  fireEvent.click(screen.getByRole("radio", { name: "Start directly", exact: false })); click("Create and start");
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ plan_mode: false }), "none");
});
it("swaps only prefixes applied by previous presets and keeps manual branch edits", async () => {
  mount({}, [preset, { ...preset, name: "Fix", branch_prefix: "fix" }, { ...preset, name: "Plain", branch_prefix: null }]);
  await ready(); change("Branch", "nested/meaningful"); change("Feature preset", "0");
  expect(screen.getByLabelText("Branch")).toHaveProperty("value", "feat/nested/meaningful");
  change("Feature preset", "1"); expect(screen.getByLabelText("Branch")).toHaveProperty("value", "fix/nested/meaningful");
  change("Feature preset", "2"); expect(screen.getByLabelText("Branch")).toHaveProperty("value", "nested/meaningful");
  change("Branch", "custom/edited"); change("Feature preset", "0");
  expect(screen.getByLabelText("Branch")).toHaveProperty("value", "feat/custom/edited");
});
it("retains Remote Control intent through edits, custom settings and project switches until explicit opt-out", async () => {
  const { onSubmit } = mount({ projects }, [{ ...preset, remote_control: true, plan_mode: false }]);
  await ready(); change("Feature preset", "0"); change("Branch", "edited"); change("Feature preset", ""); change("Project", "second");
  expect(screen.getByText(/does not support Remote Control yet/)).toBeTruthy();
  expect(screen.getByRole("button", { name: "Create and start" })).toHaveProperty("disabled", true);
  fireEvent.submit(screen.getByRole("dialog").querySelector("form")!); expect(onSubmit).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("checkbox", { name: /^Remote Control/ })); await ready(); click("Create and start");
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ project_name: "second", branch: "edited" }), "none");
});
it("never submits an unavailable preset harness or an unsupported mode", async () => {
  const { onSubmit } = mount({}, [{ ...preset, agent: "pi", mode: "vibe", plan_mode: false }, { ...preset, name: "Unsupported", mode: "vibeless", plan_mode: false }]);
  await ready(); change("Feature preset", "0");
  expect(screen.getByText(/not offered for this project/)).toBeTruthy(); expect(screen.getByLabelText("Harness")).toHaveProperty("value", "claude");
  change("Feature preset", "1"); expect(screen.getByText(/Vibeless is unavailable with Codex/)).toBeTruthy();
  fireEvent.submit(screen.getByRole("dialog").querySelector("form")!); expect(onSubmit).not.toHaveBeenCalled();
  change("Mode", "vibe"); await ready(); click("Create and start");
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ agent: "codex", mode: "vibe" }), "none");
});
it("leaves the mode unchanged and says why when a preset's mode is not listed", async () => {
  const { onSubmit } = mount({}, [{ ...preset, mode: "retired" as FeaturePreset["mode"], plan_mode: false },
    { ...preset, name: "Both", agent: "pi", mode: "retired" as FeaturePreset["mode"], plan_mode: false }]);
  await ready(); change("Mode", "supervibe"); change("Feature preset", "0");
  expect(screen.getByText(/Ship uses the retired mode, which is not available here/)).toBeTruthy();
  expect(screen.getByLabelText("Mode")).toHaveProperty("value", "supervibe");
  expect(screen.getByLabelText("Harness")).toHaveProperty("value", "codex");
  change("Feature preset", "1");
  expect(screen.getByText(/Both uses pi, which is not offered.*It also uses the retired mode/)).toBeTruthy();
  await ready(); click("Create and start");
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ agent: "codex", mode: "supervibe", review: true }), "none");
});
it("loads the chosen project's presets without discarding its edited branch", async () => {
  mount({ projects }); await ready();
  vi.mocked(invoke).mockImplementation(async (command) => command === "allowed_harnesses" ? agents.slice(0, 1)
    : command === "feature_presets_choices" ? [{ ...preset, name: "Second only", agent: "claude" }, preset] : null);
  change("Feature preset", "0"); change("Branch", "custom/retain"); change("Project", "second");
  await screen.findByRole("option", { name: "Second only" });
  expect(screen.getByLabelText("Feature preset")).toHaveProperty("value", "");
  expect(screen.getByLabelText("Branch")).toHaveProperty("value", "custom/retain");
  // Returned data reordered the old index; no preset is silently reapplied.
  expect(screen.getByLabelText("Harness").querySelectorAll("option")).toHaveLength(1);
  change("Project", "demo");
  await screen.findByRole("option", { name: "Second only" });
  expect(screen.getByLabelText("Feature preset")).toHaveProperty("value", "");
  expect(screen.getByLabelText("Branch")).toHaveProperty("value", "custom/retain");
});
it.each(["allowed_harnesses", "feature_presets_choices"])("blocks failed %s lookup until explicit successful retry", async (command) => {
  const { onSubmit, client } = mount(); await ready();
  vi.mocked(invoke).mockImplementation(async (name) => {
    if (name === command) throw { kind: "internal", message: "Unreadable config" };
    return name === "allowed_harnesses" ? agents : name === "feature_presets_choices" ? [] : null;
  });
  await client.invalidateQueries({ queryKey: [command === "allowed_harnesses" ? "allowed-harnesses" : "feature-presets"] });
  await screen.findByText(/Unreadable config/); fireEvent.submit(screen.getByRole("dialog").querySelector("form")!); expect(onSubmit).not.toHaveBeenCalled();
  vi.mocked(invoke).mockImplementation(async (name) => name === "allowed_harnesses" ? agents : name === "feature_presets_choices" ? [] : null);
  click(command === "allowed_harnesses" ? "Retry harnesses" : "Retry presets"); await ready(); click("Create and start"); expect(onSubmit).toHaveBeenCalledOnce();
});
it("blocks loading, empty harnesses and a deleted destination without falling back", async () => {
  const { onSubmit, rerender, client } = mount({ projects });
  expect(screen.getByRole("button", { name: "Create and start" })).toHaveProperty("disabled", true); await ready();
  vi.mocked(invoke).mockImplementation(async (name) => name === "allowed_harnesses" || name === "feature_presets_choices" ? [] : null);
  await client.invalidateQueries({ queryKey: ["allowed-harnesses"] }); await screen.findByText(/No allowed, installed harnesses/);
  expect(screen.getByRole("button", { name: "Create and start" })).toHaveProperty("disabled", true);
  rerender({ projects: [projects[1]] }); await screen.findByText(/Choose an available git project/);
  fireEvent.submit(screen.getByRole("dialog").querySelector("form")!); expect(onSubmit).not.toHaveBeenCalled();
  vi.mocked(invoke).mockImplementation(async (name) => name === "allowed_harnesses" ? agents : name === "feature_presets_choices" ? [] : null);
  change("Project", "second"); await ready(); click("Create and start");
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ project_name: "second", branch: "improve-the-api" }), "none");
});
it("protects unsaved fields on Escape and keeps them after cancelling discard", async () => {
  const { onCancel, onSubmit } = mount(); await ready(); change("Feature preset", "0");
  fireEvent.keyDown(window, { key: "Escape" }); expect(await screen.findByRole("dialog", { name: "Discard TODO feature changes" })).toBeTruthy();
  expect(onCancel).not.toHaveBeenCalled(); click("Keep editing"); expect(screen.getByLabelText("Branch")).toHaveProperty("value", "feat/improve-the-api");
  click("Cancel"); click("Discard changes"); expect(onCancel).toHaveBeenCalledOnce(); expect(onSubmit).not.toHaveBeenCalled();
});
it("blocks duplicate submission and dismissal throughout an outstanding launch", async () => {
  let complete!: () => void;
  const onSubmit = vi.fn(() => new Promise<void>((resolve) => { complete = resolve; }));
  const { onCancel } = mount({ onSubmit }); await ready(); click("Create and start");
  fireEvent.submit(screen.getByRole("dialog").querySelector("form")!); fireEvent.keyDown(window, { key: "Escape" });
  expect(onSubmit).toHaveBeenCalledOnce(); expect(onCancel).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Cancel" })).toHaveProperty("disabled", true); complete(); await ready();
});
