// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import ContextSettingsPanel from "../src/ContextSettingsPanel";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });
const initial = { revision: "rev1", settings: { window_limit: "500000", warning_percent: "70", critical_percent: "85" } };
function mount() {
  const onClose = vi.fn(); const onSaved = vi.fn();
  render(<ContextSettingsPanel onClose={onClose} onSaved={onSaved} />);
  return { onClose, onSaved };
}
const change = (label: string, value: string) => fireEvent.change(screen.getByLabelText(label), { target: { value } });
const click = (name: string) => fireEvent.click(screen.getByRole("button", { name }));
it("loads without writing and retains full u64 token limits through explicit IPC save", async () => {
  vi.mocked(invoke).mockResolvedValue(initial);
  const { onSaved } = mount(); await screen.findByDisplayValue("500000");
  expect(vi.mocked(invoke).mock.calls).toEqual([["context_settings_load"]]);
  change("Context window (tokens)", "18446744073709551615"); click("Save settings");
  await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenLastCalledWith("context_settings_save", { revision: "rev1", settings: { ...initial.settings, window_limit: "18446744073709551615" } });
});
it("saves a blank window override to restore harness defaults", async () => {
  vi.mocked(invoke).mockResolvedValue(initial);
  const { onSaved } = mount(); await screen.findByDisplayValue("500000");
  change("Context window (tokens)", ""); click("Save settings");
  await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenLastCalledWith("context_settings_save", { revision: "rev1", settings: { ...initial.settings, window_limit: "" } });
});
it("validates token bounds and ordered percentages without sending invalid writes", async () => {
  vi.mocked(invoke).mockResolvedValue(initial); mount(); await screen.findByDisplayValue("500000");
  for (const value of ["0", "-1", "1.5", "18446744073709551616"]) {
    change("Context window (tokens)", value);
    expect(screen.getByRole("button", { name: "Save settings" })).toHaveProperty("disabled", true);
  }
  change("Context window (tokens)", "500000");
  for (const [warning, critical] of [["0", "85"], ["70", "101"], ["85", "85"], ["90", "80"], ["1.5", "85"]]) {
    change("Warning percent", warning); change("Critical percent", critical);
    expect(screen.getByRole("button", { name: "Save settings" })).toHaveProperty("disabled", true);
  }
  expect(invoke).toHaveBeenCalledTimes(1);
});
it("retains stale-save drafts and confirms discard before close or reload", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(initial).mockRejectedValueOnce({ kind: "conflict", message: "Global config changed" })
    .mockResolvedValueOnce({ ...initial, revision: "rev2" }).mockResolvedValueOnce(initial);
  const { onClose, onSaved } = mount(); await screen.findByDisplayValue("500000");
  change("Warning percent", "60"); click("Save settings");
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Global config changed");
  expect(screen.getByLabelText("Warning percent")).toHaveProperty("value", "60");
  expect(onSaved).not.toHaveBeenCalled();
  click("Cancel"); expect(screen.getByRole("alertdialog")).toBeTruthy(); expect(onClose).not.toHaveBeenCalled();
  click("Keep editing"); click("Reload settings"); click("Discard changes");
  await waitFor(() => expect(screen.getByLabelText("Warning percent")).toHaveProperty("value", "70"));
  change("Warning percent", "55"); click("Save settings");
  await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenLastCalledWith("context_settings_save", { revision: "rev2", settings: { ...initial.settings, warning_percent: "55" } });
});
it("reports load failures and can retry", async () => {
  vi.mocked(invoke).mockRejectedValueOnce({ kind: "internal", message: "Invalid config" }).mockResolvedValueOnce(initial);
  mount(); expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Invalid config");
  click("Reload settings"); await screen.findByDisplayValue("500000");
});
it("opens on invalid stored values so they can be repaired", async () => {
  const stored = { revision: "rev1", settings: { window_limit: "0", warning_percent: "90", critical_percent: "80" } };
  vi.mocked(invoke).mockResolvedValue(stored);
  const { onSaved } = mount(); await screen.findByDisplayValue("90");
  expect(screen.getByRole("alert").textContent).toContain("critical greater than warning");
  expect(screen.getByRole("button", { name: "Save settings" })).toHaveProperty("disabled", true);
  change("Context window (tokens)", ""); change("Critical percent", "95"); click("Save settings");
  await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenLastCalledWith("context_settings_save", { revision: "rev1", settings: { window_limit: "", warning_percent: "90", critical_percent: "95" } });
});
it.each(["Cancel", "Reload settings"])("blocks duplicate writes and discard actions during a pending save after %s", async (action) => {
  let fail!: (reason: unknown) => void;
  vi.mocked(invoke).mockResolvedValueOnce(initial).mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; }));
  const { onClose, onSaved } = mount(); await screen.findByDisplayValue("500000");
  change("Warning percent", "60"); click(action); click("Save settings"); click("Save settings"); click("Cancel");
  for (const name of ["Keep editing", "Discard changes"]) {
    expect(screen.getByRole("button", { name })).toHaveProperty("disabled", true); click(name);
  }
  expect(invoke).toHaveBeenCalledTimes(2); expect(onClose).not.toHaveBeenCalled(); expect(onSaved).not.toHaveBeenCalled();
  fail({ kind: "internal", message: "Write failed" }); await screen.findByRole("alert");
  expect(screen.getByLabelText("Warning percent")).toHaveProperty("value", "60");
  click("Discard changes");
  if (action === "Cancel") expect(onClose).toHaveBeenCalledOnce();
  else await waitFor(() => expect(invoke).toHaveBeenLastCalledWith("context_settings_load"));
});
