// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import DormancySettingsPanel from "../src/DormancySettingsPanel";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });
const initial = { revision: "rev1", settings: { idle_minutes: 60, unattended_hours: 4 } };
function mount() {
  const onClose = vi.fn(); const onSaved = vi.fn();
  render(<DormancySettingsPanel onClose={onClose} onSaved={onSaved} />);
  return { onClose, onSaved };
}
const change = (label: string, value: string) => fireEvent.change(screen.getByLabelText(label), { target: { value } });
const click = (name: string) => fireEvent.click(screen.getByRole("button", { name }));
it("loads without writing and explicitly saves zero with its revision", async () => {
  vi.mocked(invoke).mockResolvedValue(initial);
  const { onSaved } = mount();
  await screen.findByDisplayValue("60");
  expect(vi.mocked(invoke).mock.calls).toEqual([["dormancy_settings_load"]]);
  change("Idle minutes", "0"); click("Save settings");
  await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenLastCalledWith("dormancy_settings_save", {
    revision: "rev1", settings: { idle_minutes: 0, unattended_hours: 4 },
  });
});
it("retains failed saves and protects cancellation and reload", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(initial).mockRejectedValueOnce({ kind: "conflict", message: "Global config changed" })
    .mockResolvedValueOnce({ ...initial, revision: "rev2" }).mockResolvedValueOnce(initial);
  const { onClose, onSaved } = mount();
  await screen.findByDisplayValue("60");
  change("Idle minutes", "15"); click("Save settings");
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Global config changed");
  expect(screen.getByLabelText("Idle minutes")).toHaveProperty("value", "15");
  expect(onSaved).not.toHaveBeenCalled();
  click("Cancel");
  expect(screen.getByRole("alertdialog")).toBeTruthy();
  expect(onClose).not.toHaveBeenCalled();
  click("Keep editing"); click("Reload settings"); click("Discard changes");
  await screen.findByDisplayValue("60");
  change("Idle minutes", "20"); click("Save settings");
  await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenLastCalledWith("dormancy_settings_save", {
    revision: "rev2", settings: { idle_minutes: 20, unattended_hours: 4 },
  });
});
it("reports load failures and validates whole safe numbers after retry", async () => {
  vi.mocked(invoke).mockRejectedValueOnce({ kind: "internal", message: "Invalid JSON" }).mockResolvedValueOnce(initial);
  mount();
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Invalid JSON");
  expect(screen.getByRole("button", { name: "Save settings" })).toHaveProperty("disabled", true);
  click("Reload settings"); await screen.findByDisplayValue("60");
  for (const value of ["", "-1", "1.5", "9007199254740992"]) {
    change("Idle minutes", value);
    expect(screen.getByRole("button", { name: "Save settings" })).toHaveProperty("disabled", true);
  }
});
it("blocks duplicate saves and close while saving", async () => {
  let finish!: (value: unknown) => void;
  vi.mocked(invoke).mockResolvedValueOnce(initial).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  const { onSaved, onClose } = mount();
  await screen.findByDisplayValue("60");
  change("Idle minutes", "30");
  click("Save settings"); click("Save settings"); click("Cancel");
  expect(invoke).toHaveBeenCalledTimes(2);
  expect(onClose).not.toHaveBeenCalled();
  finish(initial);
  await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
});
