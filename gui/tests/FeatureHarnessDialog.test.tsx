// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import FeatureHarnessDialog from "../src/FeatureHarnessDialog";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });
const target = { project_id: "p", feature_id: "f" };
const view = { target, feature_name: "Ship", current_agent: "claude", running: true, agent_rows: 2, revision: "observed",
  choices: [{ agent: "claude", name: "Claude", unavailable: null }, { agent: "opencode", name: "OpenCode", unavailable: null },
    { agent: "codex", name: "Codex", unavailable: "Unavailable in Vibeless mode" }] };
const props = () => ({ target, hasDrafts: false, sending: false, onClose: vi.fn(), onResync: vi.fn(), onChanged: vi.fn() });
const click = (name: string) => fireEvent.click(screen.getByRole("button", { name }));
const choose = () => fireEvent.change(screen.getByLabelText("Replacement harness"), { target: { value: "opencode" } });
const confirm = () => fireEvent.click(screen.getByLabelText("I understand and confirm this harness change"));

it("selection and Cancel never switch; explicit confirmation binds the loaded observation", async () => {
  const first = props(); vi.mocked(invoke).mockResolvedValue(view);
  const mounted = render(<FeatureHarnessDialog {...first} />);
  await screen.findByLabelText("Replacement harness"); choose();
  expect(screen.getByText(/all its sessions will stop/)).toBeTruthy();
  expect(screen.getByRole("button", { name: "Change harness" })).toHaveProperty("disabled", true);
  click("Cancel"); expect(first.onClose).toHaveBeenCalledOnce(); expect(invoke).toHaveBeenCalledTimes(1);
  mounted.unmount(); const next = props(); render(<FeatureHarnessDialog {...next} />);
  await screen.findByLabelText("Replacement harness"); choose(); confirm(); click("Change harness");
  await waitFor(() => expect(next.onChanged).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenLastCalledWith("feature_harness_apply", { request: { target, revision: "observed", agent: "opencode" } });
  expect(next.onResync).toHaveBeenCalledOnce();
});
it("blocks unsent drafts and sends, and selecting another harness clears consent", async () => {
  vi.mocked(invoke).mockResolvedValue(view); const base = props();
  const mounted = render(<FeatureHarnessDialog {...base} hasDrafts />);
  await screen.findByLabelText("Replacement harness"); choose(); confirm();
  expect(screen.getByRole("alert").textContent).toContain("unsent agent drafts");
  expect(screen.getByRole("button", { name: "Change harness" })).toHaveProperty("disabled", true);
  mounted.rerender(<FeatureHarnessDialog {...base} sending />);
  expect(screen.getByRole("button", { name: "Change harness" })).toHaveProperty("disabled", true);
  mounted.rerender(<FeatureHarnessDialog {...base} />);
  expect(screen.getByRole("button", { name: "Change harness" })).toHaveProperty("disabled", false);
  fireEvent.change(screen.getByLabelText("Replacement harness"), { target: { value: "claude" } });
  choose(); expect(screen.getByLabelText("I understand and confirm this harness change")).toHaveProperty("checked", false);
  expect(invoke).toHaveBeenCalledTimes(1);
});
it("single-flights a switch, disables dismissal, resyncs failures and requires fresh reload", async () => {
  let reject!: (error: unknown) => void;
  vi.mocked(invoke).mockResolvedValueOnce(view).mockImplementationOnce(() => new Promise((_, failure) => { reject = failure; })).mockResolvedValue(view);
  const callbacks = props(); render(<FeatureHarnessDialog {...callbacks} />);
  await screen.findByLabelText("Replacement harness"); choose(); confirm(); click("Change harness"); click("Change harness");
  fireEvent.keyDown(window, { key: "Escape" });
  expect(callbacks.onClose).not.toHaveBeenCalled(); expect(invoke).toHaveBeenCalledTimes(2);
  reject({ kind: "conflict", message: "Checkout changed; nothing stopped" });
  expect((await screen.findByRole("alert")).textContent).toContain("Checkout changed");
  expect(callbacks.onResync).toHaveBeenCalledOnce(); expect(callbacks.onChanged).not.toHaveBeenCalled();
  expect(screen.queryByLabelText("Replacement harness")).toBeNull();
  click("Reload"); await screen.findByLabelText("Replacement harness");
  expect(screen.getByRole("button", { name: "Change harness" })).toHaveProperty("disabled", true);
});
it("states that stopped changes do not launch and reports unavailable choices", async () => {
  vi.mocked(invoke).mockResolvedValue({ ...view, running: false, choices: [view.choices[0], view.choices[2]] });
  render(<FeatureHarnessDialog {...props()} />); await screen.findByLabelText("Replacement harness");
  expect(screen.getByText(/does not launch any sessions/)).toBeTruthy();
  expect(screen.getByText(/No other installed harness/)).toBeTruthy();
  expect(screen.getByRole("option", { name: /Codex/ })).toHaveProperty("disabled", true);
});
