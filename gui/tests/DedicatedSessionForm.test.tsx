// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import DedicatedSessionForm from "../src/DedicatedSessionForm";
import type { DedicatedCatalog, PrDestinationPrepared, PrDestinationPreview } from "../src/prDestinationApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

const workflow = { kind: "triage" as const, workflow_id: "wf" };
const identity = { pr_number: 12, head_sha: "abc1234", set_digest: "set-1" };
const catalog: DedicatedCatalog = {
  identity, feature_name: "Round totals", default_name: "PR Triage", preferred: "claude",
  agents: [
    { agent: "claude", label: "Claude", unavailable: null },
    { agent: "codex", label: "Codex", unavailable: null },
    { agent: "pi", label: "Pi", unavailable: "pi is not installed" },
  ],
};
const preview = (over: Partial<PrDestinationPreview> = {}): PrDestinationPreview => ({
  revision: "rev-1", identity, label: "PR Triage", agent: "claude", feature_name: "Round totals", workdir: "/work",
  reuses_existing: false, existing_target: null, creates_session: true, stopped: true, resource_warning: null,
  consequences: ["Adds a 'PR Triage' Claude session to Round totals.", "The agent is created stopped: no process runs and no prompt is sent. Start it from the session tab (Start) when you are ready.", "Nothing is posted to GitHub."],
  ...over,
});
const target = { project_id: "p", feature_id: "f", session_id: "s-new" };
const prepared = (over: Partial<PrDestinationPrepared> = {}): PrDestinationPrepared => ({
  status: "created", operation_id: "op", created_target: target, destination_revision: "dest-rev", destination_kind: "opened_feature",
  stopped: true, message: "Created 'PR Triage' (Claude), stopped. Nothing is running.", ...over,
});

type Previews = (agent: string, name: string | null) => PrDestinationPreview | Promise<PrDestinationPreview>;
function backend(opts: { preview?: Previews; prepare?: (args: Record<string, unknown>) => unknown; catalog?: () => unknown } = {}) {
  vi.mocked(invoke).mockImplementation(async (command: string, args?: unknown) => {
    const a = args as { request?: Record<string, unknown> };
    if (command === "pr_dedicated_catalog") return opts.catalog ? opts.catalog() : catalog;
    if (command === "pr_destination_preview") {
      const choice = a.request!.choice as { agent: string; name: string | null };
      return (opts.preview ?? ((agent: string) => preview({ agent: agent as never })))(choice.agent, choice.name);
    }
    if (command === "pr_destination_prepare") {
      const result = opts.prepare ? opts.prepare(a.request!) : prepared();
      if (result instanceof Error) throw result;
      return result;
    }
    throw new Error(command);
  });
}
const calls = (name: string) => vi.mocked(invoke).mock.calls.filter(([c]) => c === name).map(([, args]) => (args as { request: Record<string, unknown> }).request);
const createButton = () => screen.findByRole("button", { name: "Create (stopped)" }) as Promise<HTMLButtonElement>;
/** The Create button once the preview is shown and nothing blocks it. */
const create = async () => {
  const button = await createButton();
  await waitFor(() => expect(button.disabled).toBe(false));
  return button;
};

it("previews first, says the agent is stopped and how to start it, then creates once", async () => {
  backend();
  const onCreated = vi.fn();
  render(<DedicatedSessionForm workflow={workflow} expected={{ pr_number: 12, head_sha: "abc1234" }} onCreated={onCreated} onClose={vi.fn()} />);
  const button = await create();
  expect(screen.getByText("stopped", { selector: "strong" })).toBeTruthy();
  expect(screen.getByText(/choose Start/)).toBeTruthy();
  expect(screen.getByText(/Nothing is posted to GitHub\.$/, { selector: "li" })).toBeTruthy();
  expect(calls("pr_destination_prepare")).toHaveLength(0);
  fireEvent.click(button);
  await waitFor(() => expect(onCreated).toHaveBeenCalledTimes(1));
  expect(onCreated.mock.calls[0][0].created_target).toEqual(target);
  expect(calls("pr_destination_prepare")[0]).toMatchObject({ preview_revision: "rev-1", approved: false, identity,
    choice: { kind: "dedicated_session", agent: "claude", name: null } });
  expect((await screen.findByRole("status")).textContent).toContain("stopped");
  // Finished: no second creation is offered.
  expect(screen.queryByRole("button", { name: "Create (stopped)" })).toBeNull();
});

it("suppresses a double click while the creation is in flight", async () => {
  let release: (value: PrDestinationPrepared) => void = () => undefined;
  const slow = new Promise<PrDestinationPrepared>((resolve) => { release = resolve; });
  backend({ prepare: () => slow });
  const onCreated = vi.fn();
  render(<DedicatedSessionForm workflow={workflow} onCreated={onCreated} onClose={vi.fn()} />);
  const button = await create();
  fireEvent.click(button);
  fireEvent.click(button);
  fireEvent.click(button);
  expect(calls("pr_destination_prepare")).toHaveLength(1);
  expect((screen.getByRole("button", { name: "Working…" }) as HTMLButtonElement).disabled).toBe(true);
  release(prepared());
  await waitFor(() => expect(onCreated).toHaveBeenCalledTimes(1));
  expect(calls("pr_destination_prepare")).toHaveLength(1);
});

it("tells a reused session apart and offers no creation", async () => {
  backend({ preview: () => preview({ reuses_existing: true, creates_session: false, existing_target: target, consequences: ["Reuses the existing 'PR Triage' session in Round totals. Nothing is created."] }) });
  render(<DedicatedSessionForm workflow={workflow} onCreated={vi.fn()} onClose={vi.fn()} />);
  expect(await screen.findByRole("button", { name: "Use existing session" })).toBeTruthy();
  expect(screen.getByText(/already exists in Round totals/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Create (stopped)" })).toBeNull();
});

it("needs the approval checkbox before creating past the resource warning", async () => {
  backend({ preview: () => preview({ resource_warning: "Adding 'PR Triage' would exceed AMF's resource warning: 5 agents. Start anyway?" }) });
  const onCreated = vi.fn();
  render(<DedicatedSessionForm workflow={workflow} onCreated={onCreated} onClose={vi.fn()} />);
  const button = await createButton();
  const box = await screen.findByRole("checkbox", { name: /I approve despite the resource warning/ });
  expect(button.disabled).toBe(true);
  fireEvent.click(button);
  expect(calls("pr_destination_prepare")).toHaveLength(0);
  fireEvent.click(box);
  expect(button.disabled).toBe(false);
  fireEvent.click(button);
  await waitFor(() => expect(onCreated).toHaveBeenCalled());
  expect(calls("pr_destination_prepare")[0]).toMatchObject({ approved: true });
});

it("shows the approval checkbox when the backend asks for it late", async () => {
  let first = true;
  backend({ prepare: () => {
    if (first) { first = false; return Object.assign(new Error("gate"), { kind: "needs_approval", message: "Adding 'PR Triage' would exceed AMF's resource warning. Start anyway?" }); }
    return prepared();
  } });
  const onCreated = vi.fn();
  render(<DedicatedSessionForm workflow={workflow} onCreated={onCreated} onClose={vi.fn()} />);
  fireEvent.click(await create());
  const box = await screen.findByRole("checkbox", { name: /I approve despite the resource warning/ });
  expect(onCreated).not.toHaveBeenCalled();
  expect((screen.getByRole("button", { name: "Create (stopped)" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(box);
  fireEvent.click(screen.getByRole("button", { name: "Create (stopped)" }));
  await waitFor(() => expect(onCreated).toHaveBeenCalled());
  expect(calls("pr_destination_prepare").map((r) => r.approved)).toEqual([false, true]);
});

it("disables creation when the preview went stale until it is refreshed", async () => {
  let attempts = 0;
  backend({
    prepare: () => { attempts += 1; return Object.assign(new Error("stale"), { kind: "conflict", message: "The feature, session names or PR changed since the preview." }); },
    preview: () => preview({ revision: `rev-${calls("pr_destination_preview").length}` }),
  });
  const onCreated = vi.fn();
  render(<DedicatedSessionForm workflow={workflow} onCreated={onCreated} onClose={vi.fn()} />);
  fireEvent.click(await create());
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", expect.stringContaining("changed since the preview"));
  expect(screen.queryByRole("button", { name: "Create (stopped)" }) === null || (screen.getByRole("button", { name: "Create (stopped)" }) as HTMLButtonElement).disabled).toBe(true);
  expect(attempts).toBe(1);
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await create();
  expect(calls("pr_destination_preview").length).toBeGreaterThan(1);
  expect(onCreated).not.toHaveBeenCalled();
});

it("re-previews on a harness change and keeps creation disabled until the preview matches", async () => {
  backend({ preview: (agent, name) => preview({ agent: agent as never, label: name ?? "PR Triage", revision: `rev-${agent}-${name}` }) });
  render(<DedicatedSessionForm workflow={workflow} onCreated={vi.fn()} onClose={vi.fn()} />);
  await create();
  fireEvent.change(screen.getByRole("combobox", { name: "Agent" }), { target: { value: "codex" } });
  const button = screen.getByRole("button", { name: "Create (stopped)" }) as HTMLButtonElement;
  expect(button.disabled).toBe(true);
  await waitFor(() => expect(button.disabled).toBe(false));
  fireEvent.change(screen.getByLabelText(/Session name/), { target: { value: "Hotfix" } });
  expect((screen.getByRole("button", { name: "Create (stopped)" }) as HTMLButtonElement).disabled).toBe(true);
  await waitFor(() => expect((screen.getByRole("button", { name: "Create (stopped)" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Create (stopped)" }));
  await waitFor(() => expect(calls("pr_destination_prepare")).toHaveLength(1));
  expect(calls("pr_destination_prepare")[0]).toMatchObject({ preview_revision: "rev-codex-Hotfix", choice: { agent: "codex", name: "Hotfix" } });
});

it("lists an unavailable harness with its reason and never selects it", async () => {
  backend();
  render(<DedicatedSessionForm workflow={workflow} onCreated={vi.fn()} onClose={vi.fn()} />);
  await create();
  const pi = screen.getByRole("option", { name: /Pi \(unavailable\)/ }) as HTMLOptionElement;
  expect(pi.disabled).toBe(true);
  expect(screen.getByText(/pi is not installed/)).toBeTruthy();
  expect((screen.getByRole("combobox", { name: "Agent" }) as HTMLSelectElement).value).toBe("claude");
});

it("refuses to preview when the panel's PR is not the live one", async () => {
  backend();
  render(<DedicatedSessionForm workflow={workflow} expected={{ pr_number: 12, head_sha: "older" }} onCreated={vi.fn()} onClose={vi.fn()} />);
  expect((await screen.findByRole("alert")).textContent).toContain("The PR changed since this panel loaded");
  expect(calls("pr_destination_preview")).toHaveLength(0);
});

it("reports an already-running creation without claiming success", async () => {
  backend({ prepare: () => prepared({ status: "in_progress", created_target: null, message: "This session is already being created." }) });
  const onCreated = vi.fn();
  render(<DedicatedSessionForm workflow={workflow} onCreated={onCreated} onClose={vi.fn()} />);
  fireEvent.click(await create());
  expect((await screen.findByRole("alert")).textContent).toContain("already being created");
  expect(onCreated).not.toHaveBeenCalled();
});
