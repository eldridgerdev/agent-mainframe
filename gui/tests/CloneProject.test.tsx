// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import CreateProjectForm from "../src/CreateProjectForm";
import type { CloneProjectSnapshot, ClonePhase, HarnessInfo } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const supported: HarnessInfo[] = [{ slug: "claude", display_name: "Claude Code" }];
const URL = "https://github.com/o/widget.git";
const clients: QueryClient[] = [];
let emit: (snapshot: CloneProjectSnapshot) => void = () => undefined;

function snapshot(phase: ClonePhase, extra: Partial<CloneProjectSnapshot> = {}): CloneProjectSnapshot {
  return { phase, url: URL, name: "widget", dest: "/home/u/code/widget", can_start: false, can_cancel: false, ...extra };
}
const done = (blocked: boolean, status: "pass" | "warn" | "fail"): ClonePhase => ({
  kind: "preflight_done", default_branch: "main", blocked,
  checks: [
    { id: "git", status: "pass", message: "git 2.45 found", fix: null },
    { id: "gh_auth", status: status, message: "gh is not signed in", fix: status === "pass" ? null : { id: "gh_auth_login", label: "Sign in to GitHub", command_line: "gh auth login" } },
  ],
});

type Handler = (args: Record<string, unknown>) => unknown;
let handlers: Record<string, Handler>;
const calls: { command: string; args: unknown }[] = [];

beforeEach(() => {
  calls.length = 0;
  handlers = {};
  vi.mocked(listen).mockImplementation(async (_event, callback) => {
    emit = (payload) => (callback as (e: { payload: CloneProjectSnapshot }) => void)({ payload });
    return () => undefined;
  });
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    calls.push({ command, args });
    if (command === "allowed_harnesses") return supported;
    const handler = handlers[command];
    if (!handler) return null;
    const result = handler((args ?? {}) as Record<string, unknown>);
    if (result instanceof Error) throw JSON.parse(result.message);
    return result;
  });
});
afterEach(() => { cleanup(); clients.splice(0).forEach((c) => c.clear()); vi.resetAllMocks(); });

const fail = (message: string) => new Error(JSON.stringify({ kind: "invalid_input", message }));

function mount(props: Partial<React.ComponentProps<typeof CreateProjectForm>> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  clients.push(client);
  const onCloned = vi.fn();
  const onCancel = vi.fn();
  const onSubmit = vi.fn();
  render(<QueryClientProvider client={client}>
    <CreateProjectForm agents={supported} pending={false} error={null} onEdit={vi.fn()}
      onCancel={onCancel} onSubmit={onSubmit} onCloned={onCloned} {...props} />
  </QueryClientProvider>);
  return { onCloned, onCancel, onSubmit };
}
const count = (command: string) => calls.filter((c) => c.command === command).length;

async function typeUrl(value = URL) {
  handlers.clone_project_derive = () => ({ name: "widget", dest: "/home/u/code/widget" });
  fireEvent.change(screen.getByLabelText(/^Git URL/), { target: { value } });
  await waitFor(() => expect((screen.getByLabelText("Project name") as HTMLInputElement).value).toBe("widget"));
}

it("an empty URL keeps today's form and never touches the clone commands", async () => {
  const { onSubmit } = mount();
  fireEvent.change(screen.getByLabelText(/^Repository path/), { target: { value: "/code/repo" } });
  fireEvent.change(screen.getByLabelText("Project name"), { target: { value: "repo" } });
  fireEvent.click(screen.getByRole("button", { name: "Create project" }));
  expect(onSubmit).toHaveBeenCalledWith({ path: "/code/repo", project_name: "repo", preferred_agent: null, dry_run: false });
  expect(calls.filter((c) => c.command.startsWith("clone_project"))).toEqual([]);
  expect(listen).not.toHaveBeenCalled();
});

it("typing a URL asks the backend for the name and folder, and edits stick", async () => {
  mount();
  await typeUrl();
  expect((screen.getByLabelText(/^Clone into/) as HTMLInputElement).value).toBe("/home/u/code/widget");
  expect(calls.find((c) => c.command === "clone_project_derive")?.args).toEqual({ url: URL });

  fireEvent.change(screen.getByLabelText("Project name"), { target: { value: "mine" } });
  fireEvent.change(screen.getByLabelText(/^Clone into/), { target: { value: "/srv/mine" } });
  handlers.clone_project_derive = () => ({ name: "other", dest: "/home/u/code/other" });
  fireEvent.change(screen.getByLabelText(/^Git URL/), { target: { value: "https://github.com/o/other.git" } });
  await waitFor(() => expect(count("clone_project_derive")).toBe(2));
  await act(async () => { await new Promise((r) => setTimeout(r, 20)); });
  expect((screen.getByLabelText("Project name") as HTMLInputElement).value).toBe("mine");
  expect((screen.getByLabelText(/^Clone into/) as HTMLInputElement).value).toBe("/srv/mine");
});

it("shows the backend's reason for a rejected URL and blocks the check", async () => {
  mount();
  handlers.clone_project_derive = () => fail("Only https and ssh URLs can be cloned.");
  fireEvent.change(screen.getByLabelText(/^Git URL/), { target: { value: "file:///x" } });
  expect((await screen.findByRole("alert")).textContent).toBe("Only https and ssh URLs can be cloned.");
  expect((screen.getByRole("button", { name: "Check and clone" }) as HTMLButtonElement).disabled).toBe(true);
});

it("a Fail disables Start, offers a copy-only fix, and Re-check runs the checks again", async () => {
  const writeText = vi.fn(async () => undefined);
  Object.assign(navigator, { clipboard: { writeText } });
  mount();
  await typeUrl();
  handlers.clone_project_preflight = () => snapshot(done(true, "fail"), { can_start: false });
  fireEvent.click(screen.getByRole("button", { name: "Check and clone" }));
  const list = await screen.findByRole("list", { name: "Pre-flight checks" });
  expect(within(list).getAllByRole("listitem").map((li) => li.getAttribute("data-status"))).toEqual(["pass", "fail"]);
  expect((screen.getByRole("button", { name: "Start clone" }) as HTMLButtonElement).disabled).toBe(true);
  expect(calls.find((c) => c.command === "clone_project_preflight")?.args).toEqual({ url: URL, dest: "/home/u/code/widget", name: "widget" });

  fireEvent.click(screen.getByRole("button", { name: "Copy command: gh auth login" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith("gh auth login"));
  expect(calls.some((c) => c.command.includes("run"))).toBe(false);

  handlers.clone_project_preflight = () => snapshot(done(false, "pass"), { can_start: true });
  fireEvent.click(screen.getByRole("button", { name: "Re-check" }));
  await waitFor(() => expect((screen.getByRole("button", { name: "Start clone" }) as HTMLButtonElement).disabled).toBe(false));
  expect(count("clone_project_preflight")).toBe(2);
});

it("a Warn still allows Start", async () => {
  mount();
  await typeUrl();
  handlers.clone_project_preflight = () => snapshot(done(false, "warn"), { can_start: true });
  fireEvent.click(screen.getByRole("button", { name: "Check and clone" }));
  await screen.findByRole("list", { name: "Pre-flight checks" });
  expect((screen.getByRole("button", { name: "Start clone" }) as HTMLButtonElement).disabled).toBe(false);
});

it("progress arrives as events, Cancel follows can_cancel, and success selects the project", async () => {
  const { onCloned } = mount();
  await typeUrl();
  handlers.clone_project_preflight = () => snapshot(done(false, "pass"), { can_start: true });
  fireEvent.click(screen.getByRole("button", { name: "Check and clone" }));
  await screen.findByRole("button", { name: "Start clone" });

  handlers.clone_project_start = () => snapshot({ kind: "cloning", progress: null }, { can_cancel: true });
  fireEvent.click(screen.getByRole("button", { name: "Start clone" }));
  expect(await screen.findByText("Starting the clone…")).toBeTruthy();
  expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(false);

  act(() => emit(snapshot({ kind: "cloning", progress: { phase: "receiving", percent: 42, detail: "Receiving objects: 42%" } }, { can_cancel: true })));
  expect(screen.getByText("Receiving objects: 42%")).toBeTruthy();
  expect((screen.getByLabelText("Clone progress") as HTMLProgressElement).value).toBe(42);

  act(() => emit(snapshot({ kind: "registering" }, { can_cancel: false })));
  expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(true);

  act(() => emit(snapshot({ kind: "succeeded", name: "widget", dest: "/home/u/code/widget", branch: "main" })));
  await waitFor(() => expect(onCloned).toHaveBeenCalledWith({ name: "widget", dest: "/home/u/code/widget" }));
  expect(onCloned).toHaveBeenCalledTimes(1);
});

it("a failed clone returns to the form with every value kept and shows the advice", async () => {
  mount();
  await typeUrl();
  fireEvent.change(screen.getByLabelText(/^Clone into/), { target: { value: "/srv/mine" } });
  handlers.clone_project_preflight = () => snapshot(done(false, "pass"), { can_start: true });
  fireEvent.click(screen.getByRole("button", { name: "Check and clone" }));
  await screen.findByRole("button", { name: "Start clone" });
  handlers.clone_project_start = () => snapshot({ kind: "cloning", progress: null }, { can_cancel: true });
  fireEvent.click(screen.getByRole("button", { name: "Start clone" }));
  await screen.findByText("Starting the clone…");

  act(() => emit(snapshot({
    kind: "failed", message: "Authentication failed.", advice: "Sign in, then try again.",
    fixes: [{ id: "gh_auth_login", label: "Sign in to GitHub", command_line: "gh auth login" }],
  })));
  expect((await screen.findByRole("alert")).textContent).toContain("Authentication failed.");
  expect(screen.getByText("Sign in, then try again.")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Copy command: gh auth login" })).toBeTruthy();
  expect((screen.getByLabelText(/^Git URL/) as HTMLInputElement).value).toBe(URL);
  expect((screen.getByLabelText("Project name") as HTMLInputElement).value).toBe("widget");
  expect((screen.getByLabelText(/^Clone into/) as HTMLInputElement).value).toBe("/srv/mine");
  expect(count("clone_project_reset")).toBe(1);
});

it("a refused check keeps the form and its values", async () => {
  mount();
  await typeUrl();
  handlers.clone_project_preflight = () => fail("A folder already exists at /home/u/code/widget.");
  fireEvent.click(screen.getByRole("button", { name: "Check and clone" }));
  expect((await screen.findByRole("alert")).textContent).toContain("A folder already exists at /home/u/code/widget.");
  expect((screen.getByLabelText("Project name") as HTMLInputElement).value).toBe("widget");
});

it("Cancel during the clone asks the backend to cancel", async () => {
  mount();
  await typeUrl();
  handlers.clone_project_preflight = () => snapshot(done(false, "pass"), { can_start: true });
  fireEvent.click(screen.getByRole("button", { name: "Check and clone" }));
  await screen.findByRole("button", { name: "Start clone" });
  handlers.clone_project_start = () => snapshot({ kind: "cloning", progress: null }, { can_cancel: true });
  fireEvent.click(screen.getByRole("button", { name: "Start clone" }));
  await screen.findByText("Starting the clone…");
  handlers.clone_project_cancel = () => snapshot({ kind: "cancelled" });
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(count("clone_project_cancel")).toBe(1));
  await screen.findByLabelText(/^Git URL/);
  expect((screen.getByLabelText("Project name") as HTMLInputElement).value).toBe("widget");
});

const rootMissing = () => new Error(JSON.stringify({ kind: "projects_root_missing", message: "The projects folder /home/u/code doesn't exist." }));

async function reachRootPanel() {
  mount();
  await typeUrl();
  handlers.clone_project_preflight = rootMissing;
  handlers.clone_project_root_status = () => "/home/u/code";
  fireEvent.click(screen.getByRole("button", { name: "Check and clone" }));
  await screen.findByRole("group", { name: "Projects folder missing" });
}

it("a missing projects folder shows the panel, then Cancel returns to the untouched form", async () => {
  await reachRootPanel();
  expect(screen.getByText("/home/u/code")).toBeTruthy();
  expect(calls.find((c) => c.command === "clone_project_root_status")?.args).toEqual({ dest: "/home/u/code/widget" });
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("group", { name: "Projects folder missing" })).toBeNull();
  expect((screen.getByLabelText(/^Git URL/) as HTMLInputElement).value).toBe(URL);
  expect((screen.getByLabelText("Project name") as HTMLInputElement).value).toBe("widget");
  expect((screen.getByLabelText(/^Clone into/) as HTMLInputElement).value).toBe("/home/u/code/widget");
  expect(count("clone_project_create_root")).toBe(0);
  expect(count("clone_project_set_root")).toBe(0);
});

it("Create folder creates the root and re-runs the same pre-flight", async () => {
  await reachRootPanel();
  handlers.clone_project_create_root = () => "/home/u/code";
  handlers.clone_project_preflight = () => snapshot(done(false, "pass"), { can_start: true });
  fireEvent.click(screen.getByRole("button", { name: "Create folder" }));
  await screen.findByRole("list", { name: "Pre-flight checks" });
  expect(count("clone_project_create_root")).toBe(1);
  const runs = calls.filter((c) => c.command === "clone_project_preflight");
  expect(runs).toHaveLength(2);
  expect(runs[1].args).toEqual(runs[0].args);
});

it("Choose a different folder sets the root, re-derives, and keeps edited fields", async () => {
  await reachRootPanel();
  fireEvent.click(screen.getByRole("button", { name: "Choose a different folder…" }));
  const input = screen.getByDisplayValue("/home/u/code") as HTMLInputElement;
  fireEvent.change(input, { target: { value: "/srv/projects" } });
  handlers.clone_project_set_root = () => "/srv/projects";
  handlers.clone_project_derive = () => ({ name: "widget", dest: "/srv/projects/widget" });
  fireEvent.click(screen.getByRole("button", { name: "Use this folder" }));
  await waitFor(() => expect((screen.getByLabelText(/^Clone into/) as HTMLInputElement).value).toBe("/srv/projects/widget"));
  expect(calls.find((c) => c.command === "clone_project_set_root")?.args).toEqual({ path: "/srv/projects" });
  expect(count("clone_project_preflight")).toBe(1);

  // A hand-edited name survives a later root change.
  fireEvent.change(screen.getByLabelText("Project name"), { target: { value: "mine" } });
  handlers.clone_project_preflight = rootMissing;
  fireEvent.click(screen.getByRole("button", { name: "Check and clone" }));
  await screen.findByRole("group", { name: "Projects folder missing" });
  fireEvent.click(screen.getByRole("button", { name: "Choose a different folder…" }));
  handlers.clone_project_derive = () => ({ name: "other", dest: "/other/widget" });
  fireEvent.click(screen.getByRole("button", { name: "Use this folder" }));
  await waitFor(() => expect((screen.getByLabelText(/^Clone into/) as HTMLInputElement).value).toBe("/other/widget"));
  expect((screen.getByLabelText("Project name") as HTMLInputElement).value).toBe("mine");
});

it("errors from Create and Choose stay inside the panel", async () => {
  await reachRootPanel();
  handlers.clone_project_create_root = () => new Error(JSON.stringify({ kind: "conflict", message: "Something already exists there." }));
  fireEvent.click(screen.getByRole("button", { name: "Create folder" }));
  expect((await screen.findByRole("alert")).textContent).toBe("Something already exists there.");
  fireEvent.click(screen.getByRole("button", { name: "Choose a different folder…" }));
  handlers.clone_project_set_root = () => fail("Enter an absolute path.");
  fireEvent.click(screen.getByRole("button", { name: "Use this folder" }));
  await waitFor(() => expect(screen.getByRole("alert").textContent).toBe("Enter an absolute path."));
  expect(screen.getByRole("group", { name: "Projects folder missing" })).toBeTruthy();
  expect(count("clone_project_derive")).toBe(1);
});

it("Run in container tells the backend before it derives, and shows the backend's SSH refusal", async () => {
  mount();
  await typeUrl();
  expect(calls.find((c) => c.command === "clone_project_set_containerized")?.args).toEqual({ containerized: false });

  handlers.clone_project_derive = () => fail("Running in a container needs an https:// URL: the sandbox has no SSH keys or agent, by design.");
  fireEvent.click(screen.getByRole("checkbox", { name: /Run in container/ }));
  expect((await screen.findByRole("alert")).textContent).toMatch(/needs an https:\/\/ URL/);

  const order = calls.map((c) => c.command).filter((c) => c === "clone_project_set_containerized" || c === "clone_project_derive");
  const lastSet = order.lastIndexOf("clone_project_set_containerized");
  expect(order.lastIndexOf("clone_project_derive")).toBeGreaterThan(lastSet);
  expect(calls.filter((c) => c.command === "clone_project_set_containerized").at(-1)?.args).toEqual({ containerized: true });
});
