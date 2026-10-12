// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import FeaturePresetsPanel from "../src/FeaturePresetsPanel";
import { CreateFeatureForm } from "../src/App";
import type { FeaturePreset } from "../src/api";
vi.mock("../src/TerminalPane", () => ({ default: () => null }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.resetAllMocks(); });
const preset: FeaturePreset = { name: "Ship", branch_prefix: "feat", agent: "claude", mode: "vibe", review: false,
  plan_mode: false, quick_plan: false, enable_chrome: false, remote_control: false };
const view = { revision: "rev1", rows: [{ original_index: 0, preset }] };
const click = (name: string) => fireEvent.click(screen.getByRole("button", { name }));
const change = (label: string, value: string) => fireEvent.change(screen.getByLabelText(label), { target: { value } });

it("saves every known field with original identity only on explicit Save", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(view).mockImplementationOnce(async (_, payload) => ({ ...payload as object, revision: "rev2" }));
  render(<FeaturePresetsPanel onClose={vi.fn()} />);
  await screen.findByLabelText("Preset name");
  change("Preset name", "Renamed"); change("Branch prefix", "fix"); change("Harness", "opencode"); change("Mode", "supervibe"); change("Planning", "quick");
  for (const label of ["Review", "Chrome", "Remote Control"]) fireEvent.click(screen.getByLabelText(label));
  expect(invoke).toHaveBeenCalledTimes(1);
  click("Save presets");
  await waitFor(() => expect(invoke).toHaveBeenLastCalledWith("feature_presets_save", { revision: "rev1", rows: [{ original_index: 0,
    preset: { ...preset, name: "Renamed", branch_prefix: "fix", agent: "opencode", mode: "supervibe", plan_mode: true, quick_plan: true, review: true, enable_chrome: true, remote_control: true } }] }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Save presets" })).toHaveProperty("disabled", true));
});
it("retains failed drafts, confirms cancel/reload, and stages delete/add without writes", async () => {
  const onClose = vi.fn();
  vi.mocked(invoke).mockResolvedValueOnce(view).mockRejectedValueOnce({ kind: "conflict", message: "Global config changed" }).mockResolvedValueOnce(view);
  render(<FeaturePresetsPanel onClose={onClose} />);
  await screen.findByLabelText("Preset name"); change("Preset name", "Edited"); click("Save presets");
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Global config changed");
  click("Cancel"); expect(screen.getByRole("alertdialog")).toBeTruthy(); expect(onClose).not.toHaveBeenCalled();
  click("Keep editing"); expect(screen.getByLabelText("Preset name")).toHaveProperty("value", "Edited");
  click("Reload presets"); click("Discard changes");
  await waitFor(() => expect(screen.getByLabelText("Preset name")).toHaveProperty("value", "Ship"));
  click("Delete preset"); click("Add preset");
  expect(screen.getByRole("button", { name: "Save presets" })).toHaveProperty("disabled", true);
  change("Preset name", "New"); expect(invoke).toHaveBeenCalledTimes(3);
  click("Cancel"); click("Discard changes"); expect(onClose).toHaveBeenCalledOnce();
});
it("refuses duplicate names and locks discard controls during a pending save", async () => {
  let resolve!: (value: unknown) => void;
  vi.mocked(invoke).mockResolvedValueOnce(view).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
  render(<FeaturePresetsPanel onClose={vi.fn()} />);
  await screen.findByLabelText("Preset name"); click("Add preset"); change("Preset name", "Ship");
  expect(screen.getByRole("button", { name: "Save presets" })).toHaveProperty("disabled", true);
  change("Preset name", "New"); click("Cancel"); click("Save presets"); click("Save presets");
  for (const name of ["Keep editing", "Discard changes"]) expect(screen.getByRole("button", { name })).toHaveProperty("disabled", true);
  expect(invoke).toHaveBeenCalledTimes(2); resolve(view);
  await waitFor(() => expect(screen.getByRole("button", { name: "Save presets" })).toHaveProperty("disabled", true));
});

function creation(selected: FeaturePreset) {
  vi.mocked(invoke).mockImplementation(async (command) => command === "feature_presets_choices" ? [selected] : null);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } }); clients.push(client);
  const onSubmit = vi.fn();
  render(<QueryClientProvider client={client}><CreateFeatureForm projectId="one" projectName="one" isGit defaultUseWorktree
    agents={[{ slug: "claude", display_name: "Claude" }]} modes={[{ slug: "vibe", display_name: "Vibe", description: "" }]}
    pending={false} onCancel={vi.fn()} onSubmit={onSubmit} /></QueryClientProvider>);
  return onSubmit;
}
it("prefills presets but preserves subsequent edits and quick-plan selection at submission", async () => {
  const onSubmit = creation({ ...preset, review: true, enable_chrome: true, plan_mode: true, quick_plan: true });
  await screen.findByRole("option", { name: "Ship" }); change("Feature preset", "0");
  expect(screen.getByLabelText("Branch / feature name")).toHaveProperty("value", "feat/");
  change("Branch / feature name", "custom/edited"); fireEvent.click(screen.getByRole("checkbox", { name: "Chrome" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Create and plan" })).toHaveProperty("disabled", false));
  click("Create and plan");
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ branch: "custom/edited", review: true, enable_chrome: false, plan_mode: true }), "quick");
});
it("never silently drops remote intent after field edits or switching to custom settings", async () => {
  const onSubmit = creation({ ...preset, remote_control: true });
  await screen.findByRole("option", { name: "Ship" }); change("Feature preset", "0");
  change("Branch / feature name", "edited"); change("Feature preset", "");
  expect(screen.getByRole("alert").textContent).toContain("does not support Remote Control");
  expect(screen.getByRole("button", { name: "Create feature" })).toHaveProperty("disabled", true);
  fireEvent.submit(screen.getByRole("dialog").querySelector("form")!); expect(onSubmit).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("checkbox", { name: /^Remote Control/ }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Create feature" })).toHaveProperty("disabled", false));
  click("Create feature"); expect(onSubmit).toHaveBeenCalledOnce();
});
