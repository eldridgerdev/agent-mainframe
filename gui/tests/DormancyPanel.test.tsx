// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import DormancyPanel, { humanize } from "../src/DormancyPanel";
import type { DormancyStopResult, DormancyView, DormantFeatureView } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.clearAllMocks(); });

function row(id: string, name: string, idle: number, unattended: number, extra: Partial<DormantFeatureView> = {}): DormantFeatureView {
  return {
    project_name: "Billing", workdir: `/work/${id}`, is_worktree: true, editor_alive: false,
    idle_secs: idle, unattended_secs: unattended, ...extra,
    observation: {
      target: { project_id: "project", feature_id: id }, feature_name: name, tmux_session: `amf-${id}`,
      last_activity: "2026-10-05T08:00:00Z", last_accessed: "2026-10-03T12:00:00Z",
    },
  };
}
const retry = row("retry", "Retry webhooks", 5 * 3600, 2 * 86_400, { editor_alive: true });
const docs = row("docs", "Docs refresh", 2 * 3600, 9 * 3600, { is_worktree: false });
const view = (overrides: Partial<DormancyView> = {}): DormancyView => ({
  enabled: true, idle_minutes: 60, unattended_hours: 4, kill_editor_on_stop: true,
  checked_at: "2026-10-05T10:00:00Z", features: [retry, docs], ...overrides,
});

type Handler = (command: string, args: unknown) => unknown;
function mock(handler: Handler) {
  vi.mocked(invoke).mockImplementation(async (command, args) => handler(command, args));
}
function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  const onClose = vi.fn(); const onOpenFeature = vi.fn();
  render(<QueryClientProvider client={client}><DormancyPanel onClose={onClose} onOpenFeature={onOpenFeature} /></QueryClientProvider>);
  return { onClose, onOpenFeature };
}
const stopCalls = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "dormancy_stop");
const button = (name: string | RegExp) => screen.getByRole("button", { name }) as HTMLButtonElement;

it("explains why each feature is listed and stops nothing until the user confirms", async () => {
  mock((command) => { if (command === "dormancy_load") return view(); throw new Error(command); });
  mount();
  expect(await screen.findByText("Retry webhooks")).toBeTruthy();
  expect(screen.getByText(/idle over 60m and not opened for over 4h/)).toBeTruthy();
  const first = within(screen.getAllByRole("listitem")[0]);
  expect(first.getByText("5h")).toBeTruthy();
  expect(first.getByText("2d")).toBeTruthy();
  expect(first.getByText(/no agent output since/)).toBeTruthy();
  expect(first.getByText(/last opened/)).toBeTruthy();
  expect(first.getByText("Editor open")).toBeTruthy();
  expect(first.getByText("Worktree")).toBeTruthy();
  expect(button("Stop selected (0)").disabled).toBe(true);

  fireEvent.click(screen.getByRole("checkbox", { name: "Select Retry webhooks" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "Select Docs refresh" }));
  fireEvent.click(button("Stop selected (2)"));
  const confirm = within(screen.getByRole("alertdialog", { name: "Confirm stopping dormant features" }));
  expect(confirm.getByText("Retry webhooks")).toBeTruthy();
  expect(confirm.getByText("(amf-docs)")).toBeTruthy();
  expect(confirm.getByText(/Editor windows AMF opened for these features are closed too/)).toBeTruthy();
  expect(confirm.getByText(/sharing its VS Code instance with other windows is left running/)).toBeTruthy();
  expect(confirm.getByText(/Each one is checked again first/)).toBeTruthy();
  fireEvent.click(button("Back"));
  expect(screen.getByRole("checkbox", { name: "Select Retry webhooks" })).toHaveProperty("checked", true);
  expect(stopCalls()).toHaveLength(0);
});

it("sends the listed observations exactly once and reports every outcome with the editor cleanup", async () => {
  let release: (results: DormancyStopResult[]) => void = () => {};
  mock((command) => {
    if (command === "dormancy_load") return view();
    if (command === "dormancy_stop") return new Promise((resolve) => { release = resolve; });
    throw new Error(command);
  });
  mount();
  fireEvent.click(await screen.findByRole("checkbox", { name: "Select Retry webhooks" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "Select Docs refresh" }));
  fireEvent.click(button("Stop selected (2)"));
  fireEvent.click(button("Stop 2 features"));
  fireEvent.click(button("Stop 2 features"));
  await waitFor(() => expect(button("Stop 2 features").disabled).toBe(true));
  expect(stopCalls()).toHaveLength(1);
  expect(stopCalls()[0][1]).toEqual({ selection: [retry.observation, docs.observation] });

  release([
    { target: retry.observation.target, feature_name: "Retry webhooks", outcome: "stopped", editors: {
      killed: [{ name: "VS Code", processes: 3 }],
      skipped: [{ name: "VS Code", reason: "AMF did not open this window", deliberate: true },
        { name: "Neovide", reason: "already closed", deliberate: false }],
      pending: ["VS Code"], summary: "closed 1 editor (3 processes)" } },
    { target: docs.observation.target, feature_name: "Docs refresh", outcome: "refused", reason: "opened",
      message: "It was opened in AMF after the list was loaded" },
  ]);
  const results = within(await screen.findByRole("region", { name: "Stop results" }));
  expect(results.getByText("stopped")).toBeTruthy();
  expect(results.getByText("Closed VS Code (3 processes ended)")).toBeTruthy();
  expect(results.getByText("Left VS Code running: AMF did not open this window")).toBeTruthy();
  expect(results.getByText("Neovide had already closed")).toBeTruthy();
  expect(results.getByText(/VS Code is still opening; AMF closes it once it can identify the window/)).toBeTruthy();
  expect(results.getByText("not stopped: It was opened in AMF after the list was loaded")).toBeTruthy();
  expect(stopCalls()).toHaveLength(1);
});

it("says when editor cleanup is off, both in the confirmation and the result", async () => {
  mock((command) => {
    if (command === "dormancy_load") return view({ kill_editor_on_stop: false, features: [docs] });
    if (command === "dormancy_stop") return [{ target: docs.observation.target, feature_name: "Docs refresh", outcome: "stopped", editors: null }];
    throw new Error(command);
  });
  mount();
  fireEvent.click(await screen.findByRole("checkbox", { name: "Select Docs refresh" }));
  fireEvent.click(button("Stop selected (1)"));
  expect(screen.getByText(/Editor cleanup is off \(kill_editor_on_stop\)/)).toBeTruthy();
  fireEvent.click(button("Stop 1 feature"));
  expect(await screen.findByText("Editor cleanup is off; no editor was examined.")).toBeTruthy();
});

it("deselects features a refresh no longer lists and re-observes the rest", async () => {
  const later = { ...retry, observation: { ...retry.observation, last_activity: "2026-10-05T09:00:00Z" } };
  let loads = 0;
  mock((command) => {
    if (command === "dormancy_load") return ++loads === 1 ? view() : view({ features: [later] });
    if (command === "dormancy_stop") return [];
    throw new Error(command);
  });
  mount();
  fireEvent.click(await screen.findByRole("checkbox", { name: "Select Retry webhooks" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "Select Docs refresh" }));
  fireEvent.click(button("Refresh"));
  expect(await screen.findByText("1 selected feature(s) are no longer dormant and were deselected.")).toBeTruthy();
  expect(screen.queryByText("Docs refresh")).toBeNull();
  fireEvent.click(button("Stop selected (1)"));
  fireEvent.click(button("Stop 1 feature"));
  await waitFor(() => expect(stopCalls()).toHaveLength(1));
  expect(stopCalls()[0][1]).toEqual({ selection: [later.observation] });
});

it("keeps the confirmation and selection when the stop request fails", async () => {
  mock((command) => {
    if (command === "dormancy_load") return view();
    if (command === "dormancy_stop") throw { kind: "internal", message: "database is locked" };
    throw new Error(command);
  });
  mount();
  fireEvent.click(await screen.findByRole("checkbox", { name: "Select Retry webhooks" }));
  fireEvent.click(button("Stop selected (1)"));
  fireEvent.click(button("Stop 1 feature"));
  expect((await screen.findByRole("alert")).textContent).toContain("database is locked");
  expect(button("Stop 1 feature").disabled).toBe(false);
});

it("shows the switched-off and nothing-dormant states, and opens a feature on request", async () => {
  mock((command) => { if (command === "dormancy_load") return view({ enabled: false, features: [] }); throw new Error(command); });
  mount();
  expect(await screen.findByText("Dormancy detection is off")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /Stop selected/ })).toBeNull();
  cleanup();

  mock((command) => { if (command === "dormancy_load") return view({ features: [] }); throw new Error(command); });
  mount();
  expect(await screen.findByText("Nothing is dormant right now")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /Stop selected/ })).toBeNull();
  cleanup();

  mock((command) => { if (command === "dormancy_load") return view(); throw new Error(command); });
  const { onOpenFeature } = mount();
  await screen.findByText("Retry webhooks");
  fireEvent.click(within(screen.getAllByRole("listitem")[0]).getByRole("button", { name: "Open" }));
  expect(onOpenFeature).toHaveBeenCalledWith(retry.observation.target);
  expect(stopCalls()).toHaveLength(0);
});

it("humanizes ages the way the TUI list does", () => {
  expect(humanize(59)).toBe("0m");
  expect(humanize(3599)).toBe("59m");
  expect(humanize(3600)).toBe("1h");
  expect(humanize(86_400 * 3 + 5)).toBe("3d");
});
