// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import AgentAvailabilityPanel from "../src/AgentAvailabilityPanel";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });
const initial = { revision: "rev1", settings: { allowed_agents: ["claude", "codex", "opencode", "pi"] } };
function mount() {
  const onClose = vi.fn(); const onSaved = vi.fn();
  render(<AgentAvailabilityPanel onClose={onClose} onSaved={onSaved} />);
  return { onClose, onSaved };
}
const toggle = (label: string) => fireEvent.click(screen.getByLabelText(label));
const click = (name: string) => fireEvent.click(screen.getByRole("button", { name }));
it("loads without writing and saves only explicit agent choices with revision", async () => {
  vi.mocked(invoke).mockResolvedValue(initial);
  const { onSaved } = mount();
  await screen.findByLabelText("Codex");
  expect(vi.mocked(invoke).mock.calls).toEqual([["agent_availability_load"]]);
  // An open TUI holds its own copy; the panel must not claim otherwise.
  expect(screen.getByText(/Restart an already open TUI/)).toBeTruthy();
  toggle("Claude Code"); toggle("OpenCode"); toggle("Pi"); click("Save settings");
  await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenLastCalledWith("agent_availability_save", {
    revision: "rev1", settings: { allowed_agents: ["codex"] },
  });
});
it("retains failed drafts and confirms discard before reload or close", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(initial).mockRejectedValueOnce({ kind: "conflict", message: "Global config changed" })
    .mockResolvedValueOnce({ ...initial, revision: "rev2" }).mockResolvedValueOnce(initial);
  const { onClose, onSaved } = mount();
  await screen.findByLabelText("Codex");
  toggle("Claude Code"); click("Save settings");
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Global config changed");
  expect(screen.getByLabelText("Claude Code")).toHaveProperty("checked", false);
  expect(onSaved).not.toHaveBeenCalled();
  click("Cancel"); expect(screen.getByRole("alertdialog")).toBeTruthy();
  expect(onClose).not.toHaveBeenCalled();
  click("Keep editing"); click("Reload settings"); click("Discard changes");
  await waitFor(() => expect(screen.getByLabelText("Claude Code")).toHaveProperty("checked", true));
  toggle("Pi"); click("Save settings");
  await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenLastCalledWith("agent_availability_save", {
    revision: "rev2", settings: { allowed_agents: ["claude", "codex", "opencode"] },
  });
});
it("reports invalid config and allows retry while refusing an empty selection", async () => {
  vi.mocked(invoke).mockRejectedValueOnce({ kind: "internal", message: "Invalid JSON" }).mockResolvedValueOnce(initial);
  mount(); expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Invalid JSON");
  click("Reload settings"); await screen.findByLabelText("Codex");
  for (const label of ["Claude Code", "Codex", "OpenCode", "Pi"]) toggle(label);
  expect(screen.getByRole("alert")).toHaveProperty("textContent", "Select at least one agent.");
  expect(screen.getByRole("button", { name: "Save settings" })).toHaveProperty("disabled", true);
});
it.each(["Cancel", "Reload settings"])("blocks duplicates and discard actions during pending save after %s", async (action) => {
  let fail!: (reason: unknown) => void;
  vi.mocked(invoke).mockResolvedValueOnce(initial).mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; }));
  const { onClose, onSaved } = mount();
  await screen.findByLabelText("Codex"); toggle("Pi"); click(action); click("Save settings");
  click("Save settings"); click("Cancel");
  for (const name of ["Keep editing", "Discard changes"]) {
    expect(screen.getByRole("button", { name })).toHaveProperty("disabled", true); click(name);
  }
  expect(invoke).toHaveBeenCalledTimes(2); expect(onClose).not.toHaveBeenCalled();
  expect(onSaved).not.toHaveBeenCalled();
  fail({ kind: "internal", message: "Cannot write settings" });
  await screen.findByRole("alert"); expect(screen.getByLabelText("Pi")).toHaveProperty("checked", false);
  click("Discard changes");
  if (action === "Cancel") expect(onClose).toHaveBeenCalledOnce();
  else await waitFor(() => expect(invoke).toHaveBeenLastCalledWith("agent_availability_load"));
});

const projectInitial = { revision: "project-rev", settings: { allowed_agents: null as string[] | null },
  inherited_agents: ["claude", "pi"], project_name: "demo", repo: "/demo", config_path: "/demo/amf.json", source_path: null };
function mountProject() {
  const onClose = vi.fn(); const onSaved = vi.fn();
  render(<AgentAvailabilityPanel projectId="project-id" onClose={onClose} onSaved={onSaved} />);
  return { onClose, onSaved };
}
it("distinguishes inherited choices from explicit project override and scopes save by stable project ID", async () => {
  vi.mocked(invoke).mockResolvedValue(projectInitial);
  const { onSaved } = mountProject();
  expect(await screen.findByRole("radio", { name: "Use global setting (clear project override)" })).toHaveProperty("checked", true);
  expect(invoke).toHaveBeenCalledWith("project_agent_availability_load", { projectId: "project-id" });
  expect(screen.getByLabelText("Claude Code")).toHaveProperty("checked", true);
  expect(screen.getByLabelText("Pi")).toHaveProperty("disabled", true);
  expect(screen.getByRole("button", { name: "Save settings" })).toHaveProperty("disabled", true);
  toggle("Use project override"); toggle("Claude Code"); toggle("Pi"); toggle("Codex"); click("Save settings");
  await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenLastCalledWith("project_agent_availability_save", {
    projectId: "project-id", revision: "project-rev", settings: { allowed_agents: ["codex"] },
  });
});
it("clears project override explicitly and confirms discard before restoring saved override", async () => {
  vi.mocked(invoke).mockResolvedValue({ ...projectInitial, settings: { allowed_agents: ["codex"] } });
  const { onClose, onSaved } = mountProject();
  await screen.findByLabelText("Codex"); toggle("Use global setting (clear project override)");
  expect(screen.getByLabelText("Claude Code")).toHaveProperty("checked", true);
  expect(screen.getByLabelText("Codex")).toHaveProperty("checked", false);
  click("Cancel"); expect(screen.getByRole("alertdialog")).toBeTruthy(); expect(onClose).not.toHaveBeenCalled();
  click("Keep editing"); click("Reload settings"); click("Discard changes");
  await waitFor(() => expect(screen.getByLabelText("Codex")).toHaveProperty("checked", true));
  toggle("Use global setting (clear project override)"); click("Save settings");
  await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenLastCalledWith("project_agent_availability_save", {
    projectId: "project-id", revision: "project-rev", settings: { allowed_agents: null },
  });
});
it("retains project draft after stale refusal and blocks scope changes while save is pending", async () => {
  let fail!: (reason: unknown) => void;
  vi.mocked(invoke).mockResolvedValueOnce(projectInitial).mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; }));
  const { onSaved } = mountProject();
  await screen.findByLabelText("Codex"); toggle("Use project override"); toggle("Codex"); click("Save settings");
  expect(screen.getByLabelText("Use global setting (clear project override)")).toHaveProperty("disabled", true);
  click("Save settings"); expect(invoke).toHaveBeenCalledTimes(2);
  fail({ kind: "conflict", message: "Project or agent settings changed. Reload settings before saving." });
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Project or agent settings changed. Reload settings before saving.");
  expect(screen.getByLabelText("Use project override")).toHaveProperty("checked", true);
  expect(screen.getByLabelText("Codex")).toHaveProperty("checked", true);
  expect(onSaved).not.toHaveBeenCalled();
});
