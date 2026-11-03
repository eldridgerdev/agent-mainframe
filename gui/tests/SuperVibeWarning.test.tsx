// @vitest-environment jsdom
import { creationChoices } from "./creationChoices";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import type { ReactNode } from "react";
import { CreateFeatureForm, TodoNewFeatureForm } from "../src/App";
import FeaturePresetsPanel from "../src/FeaturePresetsPanel";
import ReviewFeedbackDestinationDialog from "../src/ReviewFeedbackDestinationDialog";
import { reviewDestinationCatalog, type ReviewDestinationCatalog, type ReviewView } from "../src/api";
import { getConfirm, queryConfirm } from "./confirmCallout";

vi.mock("../src/TerminalPane", () => ({ default: () => null }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../src/api", async importOriginal => ({ ...await importOriginal<object>(), reviewDestinationCatalog: vi.fn() }));

const agents = [{ slug: "claude" as const, display_name: "Claude" }];
const modes = [
  { slug: "vibe" as const, display_name: "Vibe", description: "Asks before risky actions." },
  { slug: "supervibe" as const, display_name: "SuperVibe", description: "Skips permission prompts." },
];
const NOTE = /The agent can run any command without asking, and change or delete anything in this checkout/;
const clients: QueryClient[] = [];
beforeEach(() => {
  vi.mocked(invoke).mockImplementation(async (command) => command === "project_harness_choices" ? creationChoices(["claude"])
    : command === "allowed_harnesses" ? agents : command === "feature_presets_choices" ? [] : null);
});
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.resetAllMocks(); });

function mount(node: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  clients.push(client);
  return render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
}
const modeSelect = () => screen.getByRole("combobox", { name: /^Mode/ }) as HTMLSelectElement;
const chooseMode = (mode: string) => fireEvent.change(modeSelect(), { target: { value: mode } });
const note = () => document.querySelector("[data-supervibe-note]");

function createForm(onSubmit = vi.fn()) {
  mount(<CreateFeatureForm projectId="one" projectName="one" agents={agents} modes={modes}
    isGit defaultUseWorktree={false} pending={false} onCancel={vi.fn()} onSubmit={onSubmit} />);
  fireEvent.change(screen.getByLabelText("Branch / feature name"), { target: { value: "new-work" } });
  return onSubmit;
}
async function createButton() {
  const button = screen.getByRole("button", { name: "Create feature" }) as HTMLButtonElement;
  await waitFor(() => expect(button.disabled).toBe(false));
  return button;
}

it("shows a red risk note under the mode description only while SuperVibe is chosen", async () => {
  createForm();
  expect(note()).toBeNull();
  chooseMode("supervibe");
  expect(note()?.textContent).toMatch(NOTE);
  expect(note()?.closest(".field-hint")?.textContent).toContain("Skips permission prompts.");
  expect(note()?.className).toBe("supervibe-note");
  chooseMode("vibe");
  expect(note()).toBeNull();
});

it("asks before creating in SuperVibe, with focus on the safer choice", async () => {
  const onSubmit = createForm();
  chooseMode("supervibe");
  fireEvent.click(await createButton());
  expect(onSubmit).not.toHaveBeenCalled();
  const confirm = getConfirm();
  expect(confirm.className).toContain("callout-danger");
  expect(confirm.textContent).toMatch(NOTE);
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Choose a safer mode" }));

  fireEvent.click(screen.getByRole("button", { name: "Create in SuperVibe" }));
  expect(onSubmit).toHaveBeenCalledTimes(1);
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ branch: "new-work", mode: "supervibe" }), "none");
  expect(queryConfirm()).toBeNull();
});

it("returns to the mode field without creating when a safer mode is chosen", async () => {
  const onSubmit = createForm();
  chooseMode("supervibe");
  fireEvent.click(await createButton());
  fireEvent.click(screen.getByRole("button", { name: "Choose a safer mode" }));
  expect(queryConfirm()).toBeNull();
  expect(onSubmit).not.toHaveBeenCalled();
  expect(document.activeElement).toBe(modeSelect());

  chooseMode("vibe");
  fireEvent.click(await createButton());
  expect(queryConfirm()).toBeNull();
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ mode: "vibe" }), "none");
});

it("drops a pending SuperVibe confirmation when the mode changes away from it", async () => {
  createForm();
  chooseMode("supervibe");
  fireEvent.click(await createButton());
  expect(queryConfirm()).not.toBeNull();
  chooseMode("vibe");
  expect(queryConfirm()).toBeNull();
});

it("asks the same question before a TODO starts a SuperVibe feature", async () => {
  const onSubmit = vi.fn();
  mount(<TodoNewFeatureForm kind="launch" todoTitle="Improve API" projects={[{ id: "one", name: "one", repo: "/tmp/one", is_git: true, features: [] }]}
    agents={agents} modes={modes} pending={false} onCancel={vi.fn()} onSubmit={onSubmit} />);
  chooseMode("supervibe");
  expect(note()?.textContent).toMatch(NOTE);
  const button = screen.getByRole("button", { name: "Create and start" }) as HTMLButtonElement;
  await waitFor(() => expect(button.disabled).toBe(false));
  fireEvent.click(button);
  expect(onSubmit).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Choose a safer mode" }));
  expect(onSubmit).not.toHaveBeenCalled();
  fireEvent.click(button);
  fireEvent.click(screen.getByRole("button", { name: "Create in SuperVibe" }));
  await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ mode: "supervibe" }), "none"));
});

it("notes the SuperVibe risk on a preset that uses it", async () => {
  vi.mocked(invoke).mockResolvedValueOnce({ revision: "rev1", rows: [{ original_index: 0, preset: { name: "Ship", branch_prefix: null,
    agent: "claude", mode: "vibe", review: false, plan_mode: false, quick_plan: false, enable_chrome: false, remote_control: false } }] });
  render(<FeaturePresetsPanel onClose={vi.fn()} />);
  await screen.findByLabelText("Preset name");
  expect(note()).toBeNull();
  chooseMode("supervibe");
  expect(note()?.textContent).toMatch(NOTE);
});

it("notes the SuperVibe risk on a companion review destination, which defaults to it", async () => {
  const catalog = { sessions: [], agents: [{ agent: "claude", label: "Claude", unavailable: null }], source_path: "/source",
    source_branch: "feature", source_head: "abc123", default_branch: "feature-review-fixes", companion_available: true,
    hook_prompt: null, selected: null } as ReviewDestinationCatalog;
  vi.mocked(reviewDestinationCatalog).mockResolvedValue(catalog);
  render(<ReviewFeedbackDestinationDialog view={{ workflow_id: "review", revision: 3 } as ReviewView} error={null} onAct={vi.fn(async () => true)} onClose={vi.fn()} />);
  fireEvent.change(await screen.findByRole("combobox", { name: "Destination type" }), { target: { value: "companion" } });
  expect(modeSelect().value).toBe("supervibe");
  expect(note()?.textContent).toMatch(NOTE);
  chooseMode("vibe");
  expect(note()).toBeNull();
});
