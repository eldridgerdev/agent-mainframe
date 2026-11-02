// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import CompanionFeatureForm from "../src/CompanionFeatureForm";
import type {
  CompanionCatalog, CompanionPreview, PrDestinationOperation, PrDestinationPrepared, PrDestinationPreview,
} from "../src/prDestinationApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

const workflow = { kind: "triage" as const, workflow_id: "wf" };
const identity = { pr_number: 12, head_sha: "a".repeat(40), set_digest: "set-1" };
const catalog: CompanionCatalog = {
  identity, source_name: "Round totals", source_branch: "round-totals", project_name: "Demo", pr_branch: "round-fix",
  suggested_branch: "round-fix-triage", preferred: "claude", default_mode: "vibeless", hook: null,
  agents: [
    { agent: "claude", label: "Claude", unavailable: null },
    { agent: "codex", label: "Codex", unavailable: null },
    { agent: "pi", label: "Pi", unavailable: "pi is not installed" },
  ],
  modes: [
    { mode: "vibeless", label: "Vibeless", description: "Asks before it edits a file or runs a command.", unsupported_agents: ["codex"] },
    { mode: "vibe", label: "Vibe", description: "Edits files without asking.", unsupported_agents: [] },
    { mode: "supervibe", label: "SuperVibe", description: "Skips permission prompts.", unsupported_agents: [] },
  ],
};
const companion = (over: Partial<CompanionPreview> = {}): CompanionPreview => ({
  branch: "round-fix-triage", destination_path: "/repo/.worktrees/demo-round-fix-triage", source_path: "/repo", head_sha: "a".repeat(40),
  base_sha: "a".repeat(40), base_short: "aaaaaaa", pr_branch: "round-fix", pr_branch_on_origin: true, pr_branch_note: null, will_fetch: false,
  agent: "claude", mode: "vibeless", review: false, enable_chrome: false, hook: null, older_companions: [], ...over,
});
const preview = (over: Partial<PrDestinationPreview> = {}, extra: Partial<CompanionPreview> = {}): PrDestinationPreview => ({
  revision: "rev-1", identity, label: "New triage feature round-fix-triage (Claude, Vibeless)", agent: "claude", feature_name: "Round totals",
  workdir: "/repo/.worktrees/demo-round-fix-triage", reuses_existing: false, existing_target: null, creates_session: true, stopped: true,
  resource_warning: null,
  consequences: [
    "Creates a new Git worktree at /repo/.worktrees/demo-round-fix-triage on a new branch 'round-fix-triage', started from PR #12 head aaaaaaa.",
    "The agent is stopped: nothing runs and no prompt is sent. Start it from its session tab when you are ready.",
    "Nothing is pushed and nothing is posted to GitHub.",
  ],
  companion: companion(extra), ...over,
});
const target = { project_id: "p", feature_id: "f-new", session_id: "s-new" };
const started = (over: Partial<PrDestinationPrepared> = {}): PrDestinationPrepared => ({
  status: "started", operation_id: "op-1", created_target: null, destination_revision: null, destination_kind: "companion", stopped: true,
  message: "Creating in the background.", ...over,
});
const operation = (over: Partial<PrDestinationOperation> = {}): PrDestinationOperation => ({
  operation_id: "op-1", state: "running", phase: "fetching", cancel_requested: false, message: "Fetching PR head aaaaaaa from the remote…",
  retained_path: null, created_target: null, destination_revision: null, destination_kind: "companion", stopped: true, hook_warning: null, ...over,
});
const created = (over: Partial<PrDestinationOperation> = {}) => operation({
  state: "created", phase: "done", created_target: target, destination_revision: "dest-rev", retained_path: "/repo/.worktrees/demo-round-fix-triage",
  message: "Created 'round-fix-triage' (Claude, Vibeless) at /repo/.worktrees/demo-round-fix-triage, stopped. Nothing is running. Start it from its session tab when you are ready.", ...over,
});

interface Backend {
  catalog?: CompanionCatalog;
  preview?: (choice: { settings: { branch: string; agent: string; mode: string }; hook_choice: string | null }) => PrDestinationPreview | Error;
  prepare?: (args: Record<string, unknown>) => unknown | Promise<unknown>;
  statuses?: PrDestinationOperation[];
}
function backend(opts: Backend = {}) {
  const statuses = [...(opts.statuses ?? [created()])];
  vi.mocked(invoke).mockImplementation(async (command: string, args?: unknown) => {
    const a = args as { request?: Record<string, unknown>; operationId?: string };
    if (command === "pr_companion_catalog") return opts.catalog ?? catalog;
    if (command === "pr_destination_preview") {
      const result = (opts.preview ?? (() => preview()))(a.request!.choice as never);
      if (result instanceof Error) throw result;
      return result;
    }
    if (command === "pr_destination_prepare") {
      const result = opts.prepare ? await opts.prepare(a.request!) : started();
      if (result instanceof Error) throw result;
      return result;
    }
    if (command === "pr_destination_status") return statuses.length > 1 ? statuses.shift() : statuses[0];
    if (command === "pr_destination_cancel") return operation({ cancel_requested: true, message: "Cancelling…" });
    throw new Error(command);
  });
}
const calls = (name: string) => vi.mocked(invoke).mock.calls.filter(([c]) => c === name).map(([, args]) => args as Record<string, any>);
const createButton = () => screen.findByRole("button", { name: "Create (stopped)" }) as Promise<HTMLButtonElement>;
const create = async () => {
  const button = await createButton();
  await waitFor(() => expect(button.disabled).toBe(false));
  return button;
};
const WAIT = { timeout: 4000 };

it("previews first, states branch, base, checkout and that the agent is stopped, then creates once", async () => {
  backend();
  const onCreated = vi.fn();
  render(<CompanionFeatureForm workflow={workflow} expected={{ pr_number: 12, head_sha: "a".repeat(40) }} onCreated={onCreated} onClose={vi.fn()} />);
  const button = await create();
  expect((screen.getByLabelText(/Feature and branch name/) as HTMLInputElement).value).toBe("round-fix-triage");
  const facts = screen.getByLabelText("What will happen");
  expect(facts.textContent).toContain("round-fix-triage");
  expect(facts.textContent).toContain("/repo/.worktrees/demo-round-fix-triage");
  expect(facts.textContent).toContain("PR head aaaaaaa");
  expect(facts.textContent).toContain("PR branch (on the remote)");
  expect(facts.textContent).toContain("round-fix · found");
  expect(screen.getByText(/The agent is stopped: nothing runs/)).toBeTruthy();
  expect(screen.getByText("stopped", { selector: "strong" })).toBeTruthy();
  expect(calls("pr_destination_prepare")).toHaveLength(0);
  fireEvent.click(button);
  await waitFor(() => expect(onCreated).toHaveBeenCalledTimes(1), WAIT);
  const result = onCreated.mock.calls[0][0] as PrDestinationPrepared;
  expect(result.created_target).toEqual(target);
  expect(result.destination_revision).toBe("dest-rev");
  expect(result.destination_kind).toBe("companion");
  expect(calls("pr_destination_prepare")[0].request).toMatchObject({
    preview_revision: "rev-1", approved: false, identity,
    choice: { kind: "companion_feature", settings: { branch: "round-fix-triage", agent: "claude", mode: "vibeless", review: false, enable_chrome: false }, hook_choice: null },
  });
  expect((await screen.findByRole("status")).textContent).toContain("stopped");
  expect(screen.queryByRole("button", { name: "Create (stopped)" })).toBeNull();
  expect(screen.getByRole("button", { name: "Close" })).toBeTruthy();
});

it("suppresses a double click while the creation request is in flight", async () => {
  let release: (value: PrDestinationPrepared) => void = () => undefined;
  const slow = new Promise<PrDestinationPrepared>((resolve) => { release = resolve; });
  backend({ prepare: () => slow });
  render(<CompanionFeatureForm workflow={workflow} onCreated={vi.fn()} onClose={vi.fn()} />);
  const button = await create();
  fireEvent.click(button);
  fireEvent.click(button);
  fireEvent.click(button);
  expect(calls("pr_destination_prepare")).toHaveLength(1);
  expect((screen.getByRole("button", { name: "Working…" }) as HTMLButtonElement).disabled).toBe(true);
  release(started());
  await waitFor(() => expect(screen.queryByRole("button", { name: "Working…" })).toBeNull());
  expect(calls("pr_destination_prepare")).toHaveLength(1);
});

it("shows progress, offers Cancel, and cannot be closed while running", async () => {
  backend({ statuses: [operation(), operation({ phase: "creating_worktree", message: "Creating the worktree…" })] });
  const onClose = vi.fn();
  render(<CompanionFeatureForm workflow={workflow} onCreated={vi.fn()} onClose={onClose} />);
  fireEvent.click(await create());
  await screen.findByText(/Fetching the PR head from the remote/, undefined, WAIT);
  const close = screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement;
  expect(close.disabled).toBe(true);
  expect(close.title).toBe("Cancel the creation first");
  fireEvent.click(close);
  expect(onClose).not.toHaveBeenCalled();
  expect(screen.queryByRole("button", { name: "Create (stopped)" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Cancel creation" }));
  await waitFor(() => expect(calls("pr_destination_cancel")).toEqual([{ operationId: "op-1" }]));
  expect(await screen.findByText(/Cancelling/)).toBeTruthy();
  expect((screen.getByRole("button", { name: "Cancelling…" }) as HTMLButtonElement).disabled).toBe(true);
});

it("reports a cancelled creation, says what was kept, and lets the person try again", async () => {
  backend({ statuses: [operation({ state: "cancelled", phase: "done", cancel_requested: true, message: "Cancelled while fetching the PR head. Nothing was created." })] });
  const onCreated = vi.fn();
  render(<CompanionFeatureForm workflow={workflow} onCreated={onCreated} onClose={vi.fn()} />);
  fireEvent.click(await create());
  expect((await screen.findByText(/Creation cancelled/, undefined, WAIT)).textContent).toContain("Nothing was left behind");
  expect(onCreated).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Edit and try again" }));
  await create();
  expect(calls("pr_destination_preview").length).toBeGreaterThan(1);
});

it("names the retained checkout after a failure and never reports success", async () => {
  backend({ statuses: [operation({
    state: "failed", phase: "done", retained_path: "/repo/.worktrees/demo-x",
    message: "Triage checkout retained at /repo/.worktrees/demo-x. No feature was registered; the checkout was kept for you to inspect or remove.",
  })] });
  const onCreated = vi.fn();
  render(<CompanionFeatureForm workflow={workflow} onCreated={onCreated} onClose={vi.fn()} />);
  fireEvent.click(await create());
  const alert = await screen.findByRole("alert", undefined, WAIT);
  expect(alert.textContent).toContain("Creation failed");
  expect(alert.textContent).toContain("The checkout was kept");
  expect(screen.getByText(/No feature was registered/)).toBeTruthy();
  expect(screen.getByText("/repo/.worktrees/demo-x")).toBeTruthy();
  expect(onCreated).not.toHaveBeenCalled();
});

it("reports a hook failure as a kept companion, not a failure", async () => {
  backend({ statuses: [created({ hook_warning: "broke", message: "Created 'round-fix-triage' at /w, stopped. Its on_worktree_created hook failed (broke); the companion was kept and no agent was started." })] });
  const onCreated = vi.fn();
  render(<CompanionFeatureForm workflow={workflow} onCreated={onCreated} onClose={vi.fn()} />);
  fireEvent.click(await create());
  await waitFor(() => expect(onCreated).toHaveBeenCalledTimes(1), WAIT);
  expect((await screen.findByRole("status")).textContent).toContain("hook failed");
  expect(screen.queryByRole("alert")).toBeNull();
});

it("needs the approval checkbox before creating past the resource warning", async () => {
  backend({ preview: () => preview({ resource_warning: "Creating 'round-fix-triage' would exceed AMF's resource warning: 5 agents. Start anyway?" }) });
  const onCreated = vi.fn();
  render(<CompanionFeatureForm workflow={workflow} onCreated={onCreated} onClose={vi.fn()} />);
  const button = await createButton();
  const box = await screen.findByRole("checkbox", { name: /I approve despite the resource warning/ });
  expect(button.disabled).toBe(true);
  fireEvent.click(button);
  expect(calls("pr_destination_prepare")).toHaveLength(0);
  fireEvent.click(box);
  expect(button.disabled).toBe(false);
  fireEvent.click(button);
  await waitFor(() => expect(onCreated).toHaveBeenCalled(), WAIT);
  expect(calls("pr_destination_prepare")[0].request).toMatchObject({ approved: true });
});

it("shows the approval checkbox when the backend asks for it late", async () => {
  let first = true;
  backend({ prepare: () => {
    if (first) { first = false; return Object.assign(new Error("gate"), { kind: "needs_approval", message: "Creating 'round-fix-triage' would exceed AMF's resource warning. Start anyway?" }); }
    return started();
  } });
  const onCreated = vi.fn();
  render(<CompanionFeatureForm workflow={workflow} onCreated={onCreated} onClose={vi.fn()} />);
  fireEvent.click(await create());
  const box = await screen.findByRole("checkbox", { name: /I approve despite the resource warning/ });
  expect(onCreated).not.toHaveBeenCalled();
  fireEvent.click(box);
  fireEvent.click(screen.getByRole("button", { name: "Create (stopped)" }));
  await waitFor(() => expect(onCreated).toHaveBeenCalled(), WAIT);
  expect(calls("pr_destination_prepare").map((r) => r.request.approved)).toEqual([false, true]);
});

it("disables creation when the preview went stale until it is refreshed", async () => {
  let attempts = 0;
  backend({
    prepare: () => { attempts += 1; return Object.assign(new Error("stale"), { kind: "conflict", message: "The feature, PR, branch names or setup hook changed since the preview. Review the preview again before creating the companion." }); },
    preview: () => preview({ revision: `rev-${calls("pr_destination_preview").length}` }),
  });
  const onCreated = vi.fn();
  render(<CompanionFeatureForm workflow={workflow} onCreated={onCreated} onClose={vi.fn()} />);
  fireEvent.click(await create());
  expect((await screen.findByRole("alert")).textContent).toContain("changed since the preview");
  expect(screen.queryByRole("button", { name: "Create (stopped)" }) === null || (screen.getByRole("button", { name: "Create (stopped)" }) as HTMLButtonElement).disabled).toBe(true);
  expect(attempts).toBe(1);
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await create();
  expect(calls("pr_destination_preview").length).toBeGreaterThan(1);
  expect(onCreated).not.toHaveBeenCalled();
});

it("re-previews on every edit and keeps creation disabled until the preview matches", async () => {
  backend({ preview: ({ settings }) => preview({ revision: `rev-${settings.branch}-${settings.agent}-${settings.mode}` }, { branch: settings.branch }) });
  render(<CompanionFeatureForm workflow={workflow} onCreated={vi.fn()} onClose={vi.fn()} />);
  await create();
  fireEvent.change(screen.getByLabelText(/Feature and branch name/), { target: { value: "my-fixes" } });
  expect((screen.getByRole("button", { name: "Create (stopped)" }) as HTMLButtonElement).disabled).toBe(true);
  await waitFor(() => expect((screen.getByRole("button", { name: "Create (stopped)" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.change(screen.getByRole("combobox", { name: /^Mode/ }), { target: { value: "supervibe" } });
  expect((screen.getByRole("button", { name: "Create (stopped)" }) as HTMLButtonElement).disabled).toBe(true);
  await waitFor(() => expect((screen.getByRole("button", { name: "Create (stopped)" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Create (stopped)" }));
  await waitFor(() => expect(calls("pr_destination_prepare")).toHaveLength(1));
  expect(calls("pr_destination_prepare")[0].request).toMatchObject({
    preview_revision: "rev-my-fixes-claude-supervibe",
    choice: { settings: { branch: "my-fixes", mode: "supervibe" } },
  });
});

it("warns when the PR branch is not on the remote and keeps the form usable", async () => {
  backend({ preview: () => preview({}, {
    pr_branch_on_origin: false, will_fetch: true,
    pr_branch_note: "The PR branch 'round-fix' was not found on the remote 'origin' (for example the PR comes from a fork). A companion can still be created, but Integrate will not be able to push its fixes to the PR branch.",
  }) });
  render(<CompanionFeatureForm workflow={workflow} onCreated={vi.fn()} onClose={vi.fn()} />);
  await create();
  expect((await screen.findByRole("alert")).textContent).toContain("was not found on the remote");
  expect(screen.getByLabelText("What will happen").textContent).toContain("round-fix · not found");
  expect(screen.getByLabelText("What will happen").textContent).toContain("fetched from the remote first");
});

it("requires the setup hook's choice, shows the exact script, and sends the choice", async () => {
  const hook = { script: "./setup.sh", prompt_title: "Environment", options: ["offline", "full"], choice: null, choice_required: true };
  backend({
    catalog: { ...catalog, hook },
    preview: ({ hook_choice }) => preview({}, { hook: { ...hook, choice: hook_choice, choice_required: !hook_choice } }),
  });
  render(<CompanionFeatureForm workflow={workflow} onCreated={vi.fn()} onClose={vi.fn()} />);
  const button = await createButton();
  await screen.findByText("./setup.sh");
  expect(button.disabled).toBe(true);
  fireEvent.change(screen.getByRole("combobox", { name: /^Environment/ }), { target: { value: "full" } });
  await waitFor(() => expect(button.disabled).toBe(false), WAIT);
  expect(screen.getByLabelText("What will happen").textContent).toContain("Environment: full");
  fireEvent.click(button);
  await waitFor(() => expect(calls("pr_destination_prepare")).toHaveLength(1));
  expect(calls("pr_destination_prepare")[0].request.choice).toMatchObject({ hook_choice: "full" });
});

it("lists an unavailable harness and a mode the harness cannot run, and never selects them", async () => {
  backend();
  render(<CompanionFeatureForm workflow={workflow} onCreated={vi.fn()} onClose={vi.fn()} />);
  await create();
  expect((screen.getByRole("option", { name: /Pi \(unavailable\)/ }) as HTMLOptionElement).disabled).toBe(true);
  expect(screen.getByText(/pi is not installed/)).toBeTruthy();
  fireEvent.change(screen.getByRole("combobox", { name: "Harness" }), { target: { value: "codex" } });
  expect((screen.getByRole("option", { name: /Vibeless \(not supported by this harness\)/ }) as HTMLOptionElement).disabled).toBe(true);
});

it("refuses to preview when the panel's PR is not the live one", async () => {
  backend();
  render(<CompanionFeatureForm workflow={workflow} expected={{ pr_number: 12, head_sha: "older" }} onCreated={vi.fn()} onClose={vi.fn()} />);
  expect((await screen.findByRole("alert")).textContent).toContain("The PR changed since this panel loaded");
  expect(calls("pr_destination_preview")).toHaveLength(0);
});

it("reports a collision from the preview and offers no creation", async () => {
  backend({ preview: () => Object.assign(new Error("collision"), { kind: "conflict", message: "A local branch named 'round-fix-triage' already exists. AMF never resets or reuses an existing branch; choose another name" }) });
  render(<CompanionFeatureForm workflow={workflow} onCreated={vi.fn()} onClose={vi.fn()} />);
  expect((await screen.findByRole("alert")).textContent).toContain("never resets or reuses");
  expect((screen.getByRole("button", { name: "Create (stopped)" }) as HTMLButtonElement).disabled).toBe(true);
});
