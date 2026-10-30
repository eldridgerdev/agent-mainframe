// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import CompanionIntegrationDialog from "../src/CompanionIntegrationDialog";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
const target = { project_id: "p", feature_id: "companion" };
const plan = { revision: "revision", choice: "cherry_pick", companion_name: "Companion", source_name: "Source", companion_path: "/companion", source_path: "/source", companion_branch: "fix", source_branch: "feature", base_sha: "a".repeat(40), companion_head: "b".repeat(40), source_head: "a".repeat(40), commits: ["b".repeat(40)], source_dirty: false, companion_dirty: true, remote_tip: null, remote_display: null, ignored_collisions: [], hidden_tracked: [] };
it("previews without mutation, warns about excluded work, and confirms the exact revision once", async () => {
  let finish!: (value: string) => void;
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "companion_integration_status") return { running: false, result: null, error: null };
    if (command === "companion_integration_preview") return plan;
    if (command === "companion_integration_confirm") return new Promise(resolve => { finish = resolve; });
    throw new Error(command);
  });
  render(<CompanionIntegrationDialog target={target} onClose={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Preview integration" }));
  expect(await screen.findByText(/companion checkout has uncommitted changes/)).toBeTruthy();
  expect(screen.getAllByText(plan.companion_head).length).toBeGreaterThan(0);
  const confirm = screen.getByRole("button", { name: "Confirm cherry-pick" });
  fireEvent.click(confirm); fireEvent.click(confirm);
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "companion_integration_confirm")).toHaveLength(1);
  expect(vi.mocked(invoke).mock.calls.find(([command]) => command === "companion_integration_confirm")?.[1]).toEqual({ target, choice: "cherry_pick", revision: "revision" });
  expect((screen.getByRole("button", { name: "Close" }) as HTMLButtonElement).disabled).toBe(true);
  finish("Integrated"); await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Integrated"));
});
it("refuses dirty source and clears the preview when changing methods", async () => {
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "companion_integration_status") return { running: false, result: null, error: null };
    if (command === "companion_integration_preview") return { ...plan, source_dirty: true };
    throw new Error(command);
  });
  render(<CompanionIntegrationDialog target={target} onClose={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Preview integration" }));
  expect(await screen.findByText(/source checkout has uncommitted changes/)).toBeTruthy();
  expect((screen.getByRole("button", { name: "Confirm cherry-pick" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(screen.getByRole("combobox", { name: "Integration method" }), { target: { value: "push" } });
  expect(screen.queryByRole("button", { name: /Confirm/ })).toBeNull();
});
it("shows a running operation and its retained result after reopening", async () => {
  let calls = 0;
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "companion_integration_status") return ++calls === 1 ? { running: true, result: null, error: null } : { running: false, result: "Integration completed", error: null };
    throw new Error(command);
  });
  render(<CompanionIntegrationDialog target={target} onClose={vi.fn()} />);
  await waitFor(() => expect((screen.getByRole("button", { name: "Preview integration" }) as HTMLButtonElement).disabled).toBe(true));
  expect(await screen.findByText("Integration completed", {}, { timeout: 2500 })).toBeTruthy();
  expect((screen.getByRole("button", { name: "Preview integration" }) as HTMLButtonElement).disabled).toBe(false);
});
it("closing a preview leaves both repositories untouched", async () => {
  const close = vi.fn();
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "companion_integration_status") return { running: false, result: null, error: null };
    if (command === "companion_integration_preview") return plan;
    throw new Error(command);
  });
  render(<CompanionIntegrationDialog target={target} onClose={close} />);
  fireEvent.click(screen.getByRole("button", { name: "Preview integration" }));
  await screen.findByRole("button", { name: "Confirm cherry-pick" });
  fireEvent.click(screen.getAllByRole("button", { name: "Close" })[0]);
  expect(close).toHaveBeenCalledOnce();
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "companion_integration_confirm")).toHaveLength(0);
});
it("blocks cherry-pick when an ignored destination file would be overwritten", async () => {
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "companion_integration_status") return { running: false, result: null, error: null };
    if (command === "companion_integration_preview") return { ...plan, ignored_collisions: ["private.txt"] };
    throw new Error(command);
  });
  render(<CompanionIntegrationDialog target={target} onClose={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Preview integration" }));
  expect(await screen.findByText(/Ignored paths would be overwritten: private.txt/)).toBeTruthy();
  expect((screen.getByRole("button", { name: "Confirm cherry-pick" }) as HTMLButtonElement).disabled).toBe(true);
});
it("blocks cherry-pick when hidden tracked paths could conceal local edits", async () => {
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "companion_integration_status") return { running: false, result: null, error: null };
    if (command === "companion_integration_preview") return { ...plan, hidden_tracked: ["src/lib.rs"] };
    throw new Error(command);
  });
  render(<CompanionIntegrationDialog target={target} onClose={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Preview integration" }));
  expect(await screen.findByText(/assume-unchanged or skip-worktree.*src\/lib.rs/)).toBeTruthy();
  expect((screen.getByRole("button", { name: "Confirm cherry-pick" }) as HTMLButtonElement).disabled).toBe(true);
});
