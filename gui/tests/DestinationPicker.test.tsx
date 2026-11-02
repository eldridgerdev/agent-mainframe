// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import DestinationPicker, { DestinationRows, companionSummary } from "../src/DestinationPicker";
import type { DestinationCatalog, DestinationFeature, PrFixTarget } from "../src/prTriageApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.mocked(invoke).mockReset(); });

const session = (feature: string, id: string, kind: PrFixTarget["destination_kind"], stopped = false): PrFixTarget => ({
  target: { project_id: "p", feature_id: feature, session_id: id }, label: id, harness: "claude", stopped, unavailable: null,
  destination_revision: `${feature}-${id}`, destination_kind: kind, feature_name: feature,
});
const feature = (id: string, kind: DestinationFeature["kind"], sessions: PrFixTarget[], extra: Partial<DestinationFeature> = {}): DestinationFeature => ({
  kind, target: { project_id: "p", feature_id: id }, name: id, branch: `${id}-branch`, workdir: `/w/${id}`, running: true,
  blocked: null, sessions, companion: null, ...extra,
});
const companion = (ahead: number | null, error: string | null = null, sessions = [session("fix-triage", "triage", "companion")], extra: Partial<DestinationFeature> = {}) =>
  feature("fix-triage", "companion", sessions, { companion: { pr_branch: "feature", base_sha: "a".repeat(40), commits_ahead: ahead, commits_ahead_error: error, uncommitted_changes: false }, ...extra });
const catalog = (companions: DestinationFeature[]): DestinationCatalog => ({
  pr_number: 42, opened: feature("main", "opened_feature", [session("main", "agent", "opened_feature")]), companions,
});

it("lists this feature's sessions and the companion's sessions with branch, workdir and state", () => {
  render(<DestinationRows catalog={catalog([companion(2)])} selectedKey={null} choiceKey={null} onChoose={vi.fn()} onIntegrate={vi.fn()} />);
  expect(screen.getByText("This feature: main")).toBeTruthy();
  const group = screen.getByRole("group", { name: "Triage companion fix-triage" });
  expect(group.textContent).toContain("fix-triage-branch");
  expect(group.textContent).toContain("/w/fix-triage");
  expect(group.textContent).toContain("running");
  expect(group.textContent).toContain("triage");
});

it("disables stopped and blocked rows with a stated reason and enables running ones", () => {
  const stopped = companion(1, null, [session("fix-triage", "idle", "companion", true), session("fix-triage", "live", "companion")]);
  const onChoose = vi.fn();
  render(<DestinationRows catalog={catalog([stopped])} selectedKey={null} choiceKey={null} onChoose={onChoose} onIntegrate={vi.fn()} />);
  const buttons = screen.getAllByRole("button", { name: "Use" }) as HTMLButtonElement[];
  expect(buttons.map((b) => b.disabled)).toEqual([false, true, false]);
  expect(screen.getByText(/Stopped: start it from its session tab/)).toBeTruthy();
  fireEvent.click(buttons[2]);
  expect(onChoose).toHaveBeenCalledWith("live");
  cleanup();
  render(<DestinationRows catalog={catalog([companion(1, null, [session("fix-triage", "t", "companion")], { blocked: "Shares its tmux session with another feature; choose another destination" })])}
    selectedKey={null} choiceKey={null} onChoose={onChoose} onIntegrate={vi.fn()} />);
  expect((screen.getAllByRole("button", { name: "Use" })[1] as HTMLButtonElement).disabled).toBe(true);
});

it("flags a choice that is no longer in the catalog instead of falling back", () => {
  render(<DestinationRows catalog={catalog([companion(0)])} selectedKey={null} choiceKey="vanished" onChoose={vi.fn()} onIntegrate={vi.fn()} />);
  expect(screen.getByRole("status").textContent).toMatch(/no longer available or changed/);
  expect(screen.queryByRole("button", { name: "Selected" })).toBeNull();
});

it("shows a preselected companion session as selected without choosing or sending anything", () => {
  const onChoose = vi.fn();
  render(<DestinationRows catalog={catalog([companion(3)])} selectedKey="triage" choiceKey={null} onChoose={onChoose} onIntegrate={vi.fn()} />);
  expect((screen.getByRole("button", { name: "Selected" }) as HTMLButtonElement).getAttribute("aria-pressed")).toBe("true");
  expect(onChoose).not.toHaveBeenCalled();
  expect(vi.mocked(invoke)).not.toHaveBeenCalled();
});

it("words the companion banner, never reporting an unknown count as zero", () => {
  expect(companionSummary(companion(1))).toBe("1 commit ahead");
  expect(companionSummary(companion(0))).toBe("0 commits ahead");
  expect(companionSummary(companion(null, "The recorded base commit is not in the companion checkout"))).toBe("commits ahead unavailable");
  render(<DestinationRows catalog={catalog([companion(2)])} selectedKey={null} choiceKey={null} onChoose={vi.fn()} onIntegrate={vi.fn()} />);
  expect(screen.getByText(/Triage feature fix-triage \(2 commits ahead\)/)).toBeTruthy();
  cleanup();
  render(<DestinationRows catalog={catalog([companion(null, "Git did not finish in time")])} selectedKey={null} choiceKey={null} onChoose={vi.fn()} onIntegrate={vi.fn()} />);
  expect(screen.getByText(/Commits ahead unknown: Git did not finish in time/)).toBeTruthy();
});

it("loads the catalog read-only and the integrate link opens the existing dialog for the companion", async () => {
  vi.mocked(invoke).mockImplementation(async (command: string) => {
    if (command === "pr_destination_catalog") return catalog([companion(2)]);
    if (command === "companion_integration_status") return { running: false, result: null, error: null };
    throw new Error(command);
  });
  render(<DestinationPicker target={{ project_id: "p", feature_id: "main" }} prNumber={42} headSha={"a".repeat(40)} selectedKey={null} choiceKey={null} onChoose={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: "Integrate PR #42 fixes…" }));
  expect(await screen.findByText("Integrate companion commits")).toBeTruthy();
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("companion_integration_status", { target: { project_id: "p", feature_id: "fix-triage" } }));
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("pr_destination_catalog", { request: { target: { project_id: "p", feature_id: "main" }, pr_number: 42, head_sha: "a".repeat(40) } });
  // Nothing that writes, creates or sends was invoked.
  expect(vi.mocked(invoke).mock.calls.map(([c]) => c).filter((c) => !["pr_destination_catalog", "companion_integration_status"].includes(c as string))).toEqual([]);
});

it("shows why the catalog failed and lists nothing", async () => {
  vi.mocked(invoke).mockRejectedValue({ kind: "conflict", message: "The feature was deleted" });
  render(<DestinationPicker target={{ project_id: "p", feature_id: "main" }} prNumber={42} headSha={"a".repeat(40)} selectedKey={null} choiceKey={null} onChoose={vi.fn()} />);
  expect(await screen.findByText(/Destinations unavailable: The feature was deleted/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Use" })).toBeNull();
});

it("does not call the backend until a PR is selected", () => {
  render(<DestinationPicker target={{ project_id: "p", feature_id: "main" }} prNumber={null} headSha={"a".repeat(40)} selectedKey={null} choiceKey={null} onChoose={vi.fn()} />);
  expect(vi.mocked(invoke)).not.toHaveBeenCalled();
});

it("disables a catalog row the parent cannot resolve, with a refreshing reason, and never reports it chosen", () => {
  const onChoose = vi.fn();
  render(<DestinationRows catalog={catalog([companion(1)])} selectedKey={null} choiceKey={null} onChoose={onChoose} onIntegrate={vi.fn()}
    selectableKeys={new Set(["agent"])} />);
  const buttons = screen.getAllByRole("button", { name: "Use" }) as HTMLButtonElement[];
  expect(buttons.map((b) => b.disabled)).toEqual([false, true]);
  expect(screen.getByText(/Refreshing the agent list/)).toBeTruthy();
  fireEvent.click(buttons[1]);
  expect(onChoose).not.toHaveBeenCalled();
});

it("uses the backend's per-row unavailable reason", () => {
  const row = { ...session("fix-triage", "triage", "companion"), unavailable: "Based on an older PR head" };
  render(<DestinationRows catalog={catalog([companion(1, null, [row])])} selectedKey={null} choiceKey={null} onChoose={vi.fn()} onIntegrate={vi.fn()} />);
  expect((screen.getAllByRole("button", { name: "Use" })[1] as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText(/Based on an older PR head/)).toBeTruthy();
});

it("gates the integrate link on companion state and says why", () => {
  const onIntegrate = vi.fn();
  render(<DestinationRows catalog={catalog([companion(null, "Git did not finish in time")])} selectedKey={null} choiceKey={null} onChoose={vi.fn()} onIntegrate={onIntegrate} />);
  const link = screen.getByRole("button", { name: "Integrate PR #42 fixes…" }) as HTMLButtonElement;
  expect(link.disabled).toBe(true);
  expect(screen.getByText(/Commits ahead could not be established/)).toBeTruthy();
  cleanup();
  render(<DestinationRows catalog={catalog([companion(2, null, undefined, { blocked: "Shares its tmux session with another feature" })])} selectedKey={null} choiceKey={null} onChoose={vi.fn()} onIntegrate={onIntegrate} />);
  expect((screen.getByRole("button", { name: "Integrate PR #42 fixes…" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Integrate PR #42 fixes…" }));
  expect(onIntegrate).not.toHaveBeenCalled();
});

it("labels the commit count as committed work when the tree has uncommitted changes", () => {
  const dirty = companion(2);
  dirty.companion!.uncommitted_changes = true;
  expect(companionSummary(dirty)).toBe("2 commits ahead, committed only; uncommitted changes in its working tree");
});

it("flags a selection that disappears after the catalog refreshes", async () => {
  let rows = [session("main", "agent", "opened_feature"), session("main", "gone", "opened_feature")];
  vi.mocked(invoke).mockImplementation(async () => ({ pr_number: 42, opened: feature("main", "opened_feature", rows), companions: [] }));
  const props = { target: { project_id: "p", feature_id: "main" }, prNumber: 42, headSha: "h", selectedKey: null, choiceKey: "gone", onChoose: vi.fn() };
  const view = render(<DestinationPicker {...props} refreshKey="1" />);
  await screen.findByText("gone");
  expect(screen.queryByRole("status")).toBeNull();
  rows = [rows[0]];
  view.rerender(<DestinationPicker {...props} refreshKey="2" />);
  expect((await screen.findByRole("status")).textContent).toMatch(/no longer available or changed/);
});

it("offers the separate creation form only when the caller can open it and reports the click", () => {
  const onNewCompanion = vi.fn();
  const { rerender } = render(<DestinationRows catalog={catalog([])} selectedKey={null} choiceKey={null} onChoose={vi.fn()} onIntegrate={vi.fn()} />);
  expect(screen.queryByRole("button", { name: "New triage feature…" })).toBeNull();
  rerender(<DestinationRows catalog={catalog([])} selectedKey={null} choiceKey={null} onChoose={vi.fn()} onIntegrate={vi.fn()} onNewCompanion={onNewCompanion} />);
  fireEvent.click(screen.getByRole("button", { name: "New triage feature…" }));
  expect(onNewCompanion).toHaveBeenCalledTimes(1);
  expect(screen.getByText(/created stopped/)).toBeTruthy();
  rerender(<DestinationRows catalog={catalog([])} selectedKey={null} choiceKey={null} onChoose={vi.fn()} onIntegrate={vi.fn()} onNewCompanion={onNewCompanion} disabled />);
  expect((screen.getByRole("button", { name: "New triage feature…" }) as HTMLButtonElement).disabled).toBe(true);
});
