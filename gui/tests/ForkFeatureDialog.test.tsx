// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import ForkFeatureDialog from "../src/ForkFeatureDialog";
import type { ForkOutcome, ForkPreview } from "../src/forkApi";
import { creationChoices } from "./creationChoices";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });

const supported = [{ slug: "claude", display_name: "Claude" }, { slug: "codex", display_name: "Codex" }] as const;
const target = { project_id: "project", feature_id: "feature" };

function preview(over: Partial<ForkPreview> = {}): ForkPreview {
  return {
    token: "tok-1",
    source: {
      project_id: "project", project_name: "demo", feature_id: "feature", feature_name: "my-feat",
      branch: "my-feat", workdir: "/demo", head: "0123456789abcdef0123456789abcdef01234567",
      session: { id: "s-claude", label: "Claude 1", kind: "claude" },
    },
    suggested_branch: "my-feat-fork",
    default_agent: "claude",
    mode: "vibeless",
    changes: {
      total: 2, truncated: false,
      listed: [{ path: "src/main.rs", kind: "file" }, { path: "run.sh", kind: "executable" }],
    },
    staging_note: "Git staging choices are not copied: the new checkout starts with an empty index.",
    context: { status: "available", message: "The conversation of \"Claude 1\" will be written.", exported_chars: 120 },
    hook: null,
    ...over,
  };
}

const created: ForkOutcome = {
  status: "created", feature: { project_id: "project", feature_id: "fork-id" }, branch: "my-feat-fork",
  workdir: "/demo/.worktrees/my-feat-fork", started: false, transferred: 2, context_written: true,
  hook: null, notes: [], message: "Forked 'my-feat-fork' and kept it stopped (not started as requested).",
};

type Handler = (args: any) => unknown;
function script(handlers: Record<string, Handler>) {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "project_harness_choices") return creationChoices(["claude", "codex"]);
    const handler = handlers[command];
    if (!handler) throw new Error(`Unexpected command: ${command}`);
    return handler(args);
  });
}
const calls = (command: string) => vi.mocked(invoke).mock.calls.filter(([name]) => name === command);

function open(onClose = vi.fn(), onOpenFork = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}>
    <ForkFeatureDialog target={target} sessionId="s-claude" supported={[...supported]} onClose={onClose} onOpenFork={onOpenFork} />
  </QueryClientProvider>);
  return { onClose, onOpenFork };
}
const forkButton = () => screen.getByRole("button", { name: "Fork feature" }) as HTMLButtonElement;
const ready = async () => { await screen.findByLabelText("New branch / feature name"); await waitFor(() => expect(forkButton().disabled).toBe(false)); };

it("previews the changes, the staging rule and the context, then sends a bound confirmation", async () => {
  script({ fork_prepare: () => preview(), fork_confirm: () => created, fork_cancel: () => ({}) });
  const { onOpenFork, onClose } = open();
  await ready();

  expect(calls("fork_prepare")[0][1]).toEqual({ request: { target, session_id: "s-claude" } });
  expect(screen.getByText("2 uncommitted changes will be copied")).toBeTruthy();
  expect(screen.getByText("src/main.rs")).toBeTruthy();
  expect(screen.getByText("executable")).toBeTruthy();
  expect(screen.getByText(/staging choices are not copied/)).toBeTruthy();
  expect(screen.getByText(/0123456789/)).toBeTruthy();
  const context = screen.getByRole("checkbox", { name: /Include the conversation/ }) as HTMLInputElement;
  expect(context.checked).toBe(true);
  // Starting an agent is a visible, explicit choice.
  fireEvent.click(screen.getByRole("checkbox", { name: /Start the agent after forking/ }));

  fireEvent.click(forkButton());
  expect(await screen.findByText(/Forked 'my-feat-fork'/)).toBeTruthy();

  expect(calls("fork_confirm")[0][1]).toEqual({ request: {
    token: "tok-1",
    expected: { project_id: "project", feature_id: "feature", session_id: "s-claude", branch: "my-feat",
      head: "0123456789abcdef0123456789abcdef01234567" },
    new_branch: "my-feat-fork", agent: "claude", include_context: true, hook_choice: null,
    start_agent: false, approve_resources: false,
  } });
  expect(screen.getByText("Not started (the feature is stopped)")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Open fork" }));
  expect(onOpenFork).toHaveBeenCalledWith(created.status === "created" ? created.feature : null);
  expect(onClose).toHaveBeenCalled();
});

it.each([
  ["unsupported_harness", "Conversation export is only available for Claude Code sessions."],
  ["not_bound", "has no recorded Claude conversation yet."],
  ["unreadable", "The transcript could not be read: gone."],
] as const)("explains why no transcript is available (%s) and never sends context", async (status, message) => {
  script({ fork_prepare: () => preview({ context: { status, message, exported_chars: null } }), fork_confirm: () => created });
  open();
  await ready();

  const context = screen.getByRole("checkbox", { name: /Include the conversation/ }) as HTMLInputElement;
  expect(context.disabled).toBe(true);
  expect(context.checked).toBe(false);
  expect(screen.getByText(message)).toBeTruthy();
  fireEvent.click(forkButton());
  await screen.findByText(/Forked/);
  expect((calls("fork_confirm")[0][1] as any).request.include_context).toBe(false);
});

it("requires the project's hook choice before forking and sends it", async () => {
  script({
    fork_prepare: () => preview({ hook: { title: "Choose setup", options: ["Continue", "Other"] } }),
    fork_confirm: () => created,
  });
  open();
  await screen.findByLabelText(/Choose setup/);
  await waitFor(() => expect(screen.getByLabelText(/^Harness/)).toHaveProperty("value", "claude"));
  expect(forkButton().disabled).toBe(true);
  fireEvent.change(screen.getByLabelText(/Choose setup/), { target: { value: "Other" } });
  expect(forkButton().disabled).toBe(false);
  fireEvent.click(forkButton());
  await screen.findByText(/Forked/);
  expect((calls("fork_confirm")[0][1] as any).request.hook_choice).toBe("Other");
});

it("asks for explicit resource approval and repeats the same bound request once approved", async () => {
  let attempts = 0;
  script({
    fork_prepare: () => preview(),
    fork_status: () => ({ state: "prepared", phase: null, cancel_requested: false, outcome: null }),
    fork_confirm: () => {
      attempts++;
      if (attempts === 1) throw { kind: "needs_approval", message: "Starting fork 'my-feat-fork' would exceed AMF's resource warning: 5 agents. Start anyway?" };
      return { ...created, started: true, message: "Forked 'my-feat-fork' and started Claude." };
    },
  });
  open();
  await ready();
  fireEvent.click(forkButton());

  expect((await screen.findByRole("alert")).textContent).toContain("would exceed AMF's resource warning");
  expect(screen.queryByText(/Forked/)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Approve and fork" }));
  expect(await screen.findByText(/started Claude/)).toBeTruthy();

  const [first, second] = calls("fork_confirm").map(([, args]) => (args as any).request);
  expect(first.approve_resources).toBe(false);
  expect(second).toEqual({ ...first, approve_resources: true });
});

it("keeps the form and shows the refusal when the backend refuses a confirmation", async () => {
  script({
    fork_prepare: () => preview(),
    fork_status: () => ({ state: "prepared", phase: null, cancel_requested: false, outcome: null }),
    fork_confirm: () => { throw { kind: "conflict", message: "Feature 'my-feat-fork' already exists" }; },
  });
  open();
  await ready();
  fireEvent.click(forkButton());

  expect((await screen.findByRole("alert")).textContent).toContain("already exists");
  expect(screen.getByLabelText("New branch / feature name")).toHaveProperty("value", "my-feat-fork");
  expect(screen.queryByRole("button", { name: "Open fork" })).toBeNull();
});

it("reports a partial fork with the retained checkout and offers no way to open it as complete", async () => {
  script({
    fork_prepare: () => preview(),
    fork_confirm: () => ({
      status: "failed", phase: "transferring", feature: null,
      retained: { workdir: "/demo/.worktrees/my-feat-fork", branch: "my-feat-fork" },
      message: "Fork setup failed: destination changed. The checkout was kept for inspection and is not registered; no agent started.",
    } satisfies ForkOutcome),
  });
  open();
  await ready();
  fireEvent.click(forkButton());

  expect((await screen.findByRole("alert")).textContent).toContain("no agent started");
  expect(screen.getByText("/demo/.worktrees/my-feat-fork")).toBeTruthy();
  expect(screen.getByText("Copying working changes")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Open fork" })).toBeNull();
});

it("shows the running phase, blocks closing, and cancels through the backend", async () => {
  let finish!: (outcome: ForkOutcome) => void;
  script({
    fork_prepare: () => preview(),
    fork_confirm: () => new Promise<ForkOutcome>((resolve) => { finish = resolve; }),
    fork_status: () => ({ state: "running", phase: "hook", cancel_requested: false, outcome: null }),
    fork_cancel: () => ({ state: "running", phase: "hook", cancel_requested: true, outcome: null }),
  });
  const { onClose } = open();
  await ready();
  fireEvent.click(forkButton());

  expect(await screen.findByText("Running the worktree hook…")).toBeTruthy();
  fireEvent.keyDown(document, { key: "Escape" });
  expect(onClose).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel fork" }));
  await waitFor(() => expect(calls("fork_cancel")).toHaveLength(1));
  expect(calls("fork_cancel")[0][1]).toEqual({ token: "tok-1" });
  await act(async () => { finish({ ...created, started: false, hook: { succeeded: false, cancelled: true, detail: null },
    message: "Forked 'my-feat-fork' and kept it stopped: cancelled during the worktree hook." }); });
  expect(await screen.findByText(/cancelled during the worktree hook/)).toBeTruthy();
  expect(screen.getByText("Cancelled")).toBeTruthy();
});

it("releases the server-side preview when the dialog is cancelled or closed before it arrives", async () => {
  script({ fork_prepare: () => preview(), fork_cancel: () => ({}) });
  const { onClose } = open();
  await ready();
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(onClose).toHaveBeenCalled();
  expect(calls("fork_cancel")[0][1]).toEqual({ token: "tok-1" });

  cleanup();
  vi.mocked(invoke).mockReset();
  let arrive!: (value: ForkPreview) => void;
  script({ fork_prepare: () => new Promise<ForkPreview>((resolve) => { arrive = resolve; }), fork_cancel: () => ({}) });
  open();
  await waitFor(() => expect(calls("fork_prepare")).toHaveLength(1));
  cleanup();
  await act(async () => { arrive(preview({ token: "late" })); });
  await waitFor(() => expect(calls("fork_cancel")[0][1]).toEqual({ token: "late" }));
});

it("shows a preparation failure and can try again", async () => {
  let attempts = 0;
  script({
    fork_prepare: () => {
      attempts++;
      if (attempts === 1) throw { kind: "conflict", message: "Fork source checkout is not on its recorded branch \"my-feat\"; check the source before forking" };
      return preview();
    },
  });
  open();
  expect((await screen.findByRole("alert")).textContent).toContain("not on its recorded branch");
  fireEvent.click(screen.getByRole("button", { name: "Try again" }));
  await ready();
  expect(calls("fork_prepare")).toHaveLength(2);
});

it("will not fork with a harness the project does not offer", async () => {
  script({ fork_prepare: () => preview({ default_agent: "pi" }) });
  open();
  await screen.findByLabelText("New branch / feature name");
  await screen.findByText(/pi is not available for this project/);
  expect(forkButton().disabled).toBe(true);
  fireEvent.change(screen.getByLabelText(/^Harness/), { target: { value: "codex" } });
  expect(forkButton().disabled).toBe(false);
});

it("replaces the form with Try again when the refusal ended the preview", async () => {
  let prepares = 0;
  script({
    fork_prepare: () => { prepares++; return preview({ token: `tok-${prepares}` }); },
    fork_confirm: () => { throw { kind: "conflict", message: "Fork source changed at \"retry.py\"; prepare it again" }; },
    fork_status: () => { throw { kind: "not_found", message: "That fork preview is gone; prepare again" }; },
  });
  open();
  await ready();
  fireEvent.click(forkButton());

  expect((await screen.findByRole("alert")).textContent).toContain("prepare it again");
  expect(screen.queryByRole("button", { name: "Fork feature" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Try again" }));
  await ready();
  expect(calls("fork_prepare")).toHaveLength(2);
});
