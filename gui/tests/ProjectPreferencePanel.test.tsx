// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import ProjectPreferencePanel from "../src/ProjectPreferencePanel";
import { creationChoices } from "./creationChoices";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });
const click = (name: string) => fireEvent.click(screen.getByRole("button", { name }));
const choose = (agent: string) => fireEvent.change(screen.getByLabelText("Preferred harness"), { target: { value: agent } });

it("saves only explicit preference settings and protects cancel/reload drafts", async () => {
  const saved = vi.fn(), close = vi.fn();
  vi.mocked(invoke).mockResolvedValue(creationChoices(["claude", "opencode"]));
  render(<ProjectPreferencePanel projectId="one" onClose={close} onSaved={saved} />);
  await screen.findByLabelText("Preferred harness"); choose("opencode");
  expect(invoke).toHaveBeenCalledTimes(1); click("Cancel"); expect(screen.getByRole("alertdialog")).toBeTruthy();
  click("Keep editing"); click("Save settings");
  await waitFor(() => expect(saved).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenLastCalledWith("project_preference_save", { projectId: "one", revision: "choices-rev", settings: { preferred_agent: "opencode" } });
  expect(close).not.toHaveBeenCalled();
});

it("retains failed preference edits and reloads only after explicit discard", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(creationChoices(["claude", "pi"]))
    .mockRejectedValueOnce({ kind: "conflict", message: "Project preference changed" })
    .mockResolvedValueOnce(creationChoices(["claude", "pi"], "pi"));
  render(<ProjectPreferencePanel projectId="one" onClose={vi.fn()} onSaved={vi.fn()} />);
  await screen.findByLabelText("Preferred harness"); choose("pi"); click("Save settings");
  expect((await screen.findByRole("alert")).textContent).toContain("Project preference changed");
  expect(screen.getByLabelText("Preferred harness")).toHaveProperty("value", "pi");
  click("Reload settings"); click("Keep editing"); expect(invoke).toHaveBeenCalledTimes(2);
  click("Reload settings"); click("Discard changes");
  await waitFor(() => expect(screen.getByRole("button", { name: "Save settings" })).toHaveProperty("disabled", true));
});

it("shows unavailable saved preference without silently replacing or saving it", async () => {
  vi.mocked(invoke).mockResolvedValue(creationChoices(["claude"], "codex"));
  render(<ProjectPreferencePanel projectId="one" onClose={vi.fn()} onSaved={vi.fn()} />);
  await screen.findByLabelText("Preferred harness");
  expect(screen.getByLabelText("Preferred harness")).toHaveProperty("value", "codex");
  expect(screen.getByRole("button", { name: "Save settings" })).toHaveProperty("disabled", true);
  choose("claude"); expect(screen.getByRole("button", { name: "Save settings" })).toHaveProperty("disabled", false);
});
