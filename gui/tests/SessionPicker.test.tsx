// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import App from "../src/App";
import NewSessionDialog from "../src/NewSessionDialog";
import VscodePanel from "../src/VscodePanel";
import type { WorkspaceSnapshot } from "../src/api";
import type { CustomSessionOption, FeatureEditor, NewSessionOptions } from "../src/sessionsApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));
vi.mock("../src/TerminalPane", () => ({ default: () => <p>terminal attached</p> }));
vi.mock("../src/TodoPanel", () => ({ default: () => <p>TODO panel</p> }));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const devServer: CustomSessionOption = {
  name: "Dev server", description: "Vite on :5173", icon: "web", icon_nerd: "\u{f059f}",
  command: "npm run dev", working_dir: "web", window_name: null, pre_check: "test -f web/package.json",
  on_stop: "pkill -f vite", autolaunch: true, source: "project", revision: "rev-dev",
};
const database: CustomSessionOption = {
  name: "Database", description: null, icon: null, icon_nerd: null,
  command: "docker compose up db", working_dir: null, window_name: null,
  pre_check: "docker info >/dev/null", on_stop: null, autolaunch: false, source: "global", revision: "rev-db",
};

function pickerOptions(overrides: Partial<NewSessionOptions> = {}): NewSessionOptions {
  return {
    builtin: [
      { kind: "claude", label: "Claude", disabled: null },
      { kind: "terminal", label: "Terminal", disabled: null },
      { kind: "nvim", label: "Neovim", disabled: null },
      { kind: "vscode", label: "VS Code", disabled: "code not found in PATH" },
      { kind: "todos", label: "TODOs", disabled: null },
    ],
    custom: [devServer, database],
    config_warning: null,
    feature_stopped: false,
    ...overrides,
  };
}

function dialog(overrides: Partial<Parameters<typeof NewSessionDialog>[0]> = {}) {
  const onCreate = vi.fn();
  const props = {
    options: pickerOptions(), preferredKind: "claude" as const, busy: false, preCheckFailure: null,
    onCreate, onClose: vi.fn(), ...overrides,
  };
  const view = render(<NewSessionDialog {...props} />);
  return { onCreate, rerender: (next: Partial<typeof props>) => view.rerender(<NewSessionDialog {...props} {...next} />) };
}

it("lists configured custom sessions with what they run and creates one by revision", () => {
  const { onCreate } = dialog();

  const vscode = screen.getByRole("radio", { name: /VS Code/ }) as HTMLInputElement;
  expect(vscode.disabled).toBe(true);
  expect(screen.getByText("code not found in PATH")).toBeTruthy();

  const dev = screen.getByRole("radio", { name: "Dev server" }).closest("label")!;
  expect(within(dev).getByText("Vite on :5173")).toBeTruthy();
  expect(within(dev).getByText("npm run dev")).toBeTruthy();
  expect(within(dev).getByText("web")).toBeTruthy();
  expect(within(dev).getByText("test -f web/package.json")).toBeTruthy();
  expect(within(dev).getByText("pkill -f vite")).toBeTruthy();
  expect(within(dev).getByText("opens on create")).toBeTruthy();
  expect(within(dev).getByText("\u{f059f}")).toBeTruthy();
  const db = screen.getByRole("radio", { name: "Database" }).closest("label")!;
  expect(within(db).getByText("global")).toBeTruthy();

  fireEvent.click(screen.getByRole("radio", { name: "Database" }));
  const name = screen.getByRole("textbox", { name: /Session name/ }) as HTMLInputElement;
  expect(name.placeholder).toBe("Database");
  fireEvent.click(screen.getByRole("button", { name: "Create session" }));
  expect(onCreate).toHaveBeenCalledWith({ type: "custom", name: "Database", revision: "rev-db" }, null);
});

it("shows a failed pre-check in place until another session is chosen", () => {
  const { rerender } = dialog();
  fireEvent.click(screen.getByRole("radio", { name: "Database" }));
  rerender({ preCheckFailure: { name: "Database", preCheck: "docker info >/dev/null", output: "Cannot connect to the Docker daemon" } });

  const alert = screen.getByRole("alert");
  expect(within(alert).getByText("Database was not created: its pre-check failed")).toBeTruthy();
  expect(within(alert).getByText("Cannot connect to the Docker daemon")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Run pre-check again" })).toBeTruthy();

  fireEvent.click(screen.getByRole("radio", { name: "Dev server" }));
  expect(screen.queryByRole("alert")).toBeNull();
});

it("asks no name for VS Code or TODOs and reports an unreadable amf.json", () => {
  const { onCreate } = dialog({
    options: pickerOptions({
      builtin: [{ kind: "vscode", label: "VS Code", disabled: null }, { kind: "todos", label: "TODOs", disabled: null }],
      custom: [],
      config_warning: "/repo/amf.json could not be parsed: expected value. Only global custom sessions are listed.",
      feature_stopped: true,
    }),
  });
  expect(screen.getByText(/could not be parsed/)).toBeTruthy();
  expect(screen.queryByRole("textbox", { name: /Session name/ })).toBeNull();
  expect(screen.getByText(/this starts it, with its saved agents/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Open VS Code" }));
  expect(onCreate).toHaveBeenCalledWith({ type: "builtin", kind: "vscode" }, null);

  fireEvent.click(screen.getByRole("radio", { name: "TODOs" }));
  expect(screen.queryByText(/this starts it/)).toBeNull();
});

const editor = (id: string, state: FeatureEditor["state"]): FeatureEditor => ({
  id, name: "VS Code", state, closes_with_feature: state !== "not_owned", started_at: "2026-10-07T10:00:00Z",
});

it("closes only after confirming the listed windows and shows what was left running", async () => {
  vi.mocked(invoke).mockResolvedValue({
    already_closed: false,
    message: "closed 1 editor (2 processes); left VS Code running (AMF did not open this window)",
    editors: {
      killed: [{ name: "VS Code", processes: 2 }],
      skipped: [{ name: "VS Code", reason: "AMF did not open this window", deliberate: true }],
      pending: [], summary: null,
    },
  });
  const onClosed = vi.fn();
  render(<VscodePanel target={{ project_id: "p", feature_id: "f" }} workdir="/wt" opening={false}
    editors={[editor("owned", "open"), editor("foreign", "not_owned")]}
    onOpen={vi.fn()} onClosed={onClosed} onError={vi.fn()} />);

  expect(screen.getByText("Not AMF's")).toBeTruthy();
  expect(screen.getByText(/AMF never closes it/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /Close windows AMF opened/ }));
  expect(screen.getByText("Close this VS Code window?")).toBeTruthy();
  expect(invoke).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Close windows" }));

  await waitFor(() => expect(onClosed).toHaveBeenCalled());
  expect(invoke).toHaveBeenCalledWith("close_editors", { target: { project_id: "p", feature_id: "f" }, seen: ["owned", "foreign"] });
  expect(screen.getByText("Closed VS Code (2 processes ended)")).toBeTruthy();
  expect(screen.getByText("Left VS Code running: AMF did not open this window")).toBeTruthy();
});

it("offers no close when every window belongs to someone else", () => {
  render(<VscodePanel target={{ project_id: "p", feature_id: "f" }} workdir="/wt" opening={false}
    editors={[editor("foreign", "not_owned")]} onOpen={vi.fn()} onClosed={vi.fn()} onError={vi.fn()} />);
  expect((screen.getByRole("button", { name: /Close windows AMF opened/ }) as HTMLButtonElement).disabled).toBe(true);
});

function snapshot(extraSessions: WorkspaceSnapshot["projects"][0]["features"][0]["sessions"] = [],
  editors: FeatureEditor[] = []): WorkspaceSnapshot {
  return {
    projects: [{
      id: "project", name: "demo", repo: "/home/me/demo", is_git: true, collapsed: false,
      features: [{
        id: "feature", name: "my-feat", branch: "my-feat", workdir: "/home/me/demo/.worktrees/my-feat",
        is_worktree: true, status: "idle", agent: "claude", mode: "vibe", collapsed: true,
        sessions: [{ id: "claude", kind: "claude", label: "Claude 1", tmux_window: "claude" }, ...extraSessions],
      }],
    }],
    snapshot_at: "2026-10-07T00:00:00Z",
    stopped_session_ids: [],
    sidebar: {
      projects: { project: { repo_display: "~/demo" } },
      features: { feature: {
        workdir_display: "~/demo/.worktrees/my-feat", created_age: "3h ago", issue: null, summary_age: null,
        usage: null, pr: null, thinking: false, waiting_for_input: false, pending_input: false, editors,
      } },
      sessions: {},
    },
  };
}

function mountApp(handle: (command: string, args: Record<string, unknown>) => unknown) {
  let current = snapshot();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "get_snapshot": return current;
      case "supported_harnesses": return [{ slug: "claude", display_name: "Claude" }];
      case "supported_modes": return [{ slug: "vibe", display_name: "Vibe", description: "" }];
      case "plan_snapshot": return { active: null, precall: null };
      case "supervised_edit_counts": return [];
      case "new_session_options": return pickerOptions({
        builtin: [{ kind: "claude", label: "Claude", disabled: null }, { kind: "vscode", label: "VS Code", disabled: null }],
      });
      default: return handle(command, (args ?? {}) as Record<string, unknown>);
    }
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><App /></QueryClientProvider>);
  return { client, setCurrent: (next: WorkspaceSnapshot) => { current = next; } };
}

const tree = () => within(screen.getByRole("navigation", { name: "Workspace" }));

async function openPicker() {
  fireEvent.click(await tree().findByRole("button", { name: "my-feat", exact: true }));
  fireEvent.click(await screen.findByRole("button", { name: /New session/ }));
  return within(await screen.findByRole("dialog", { name: "New session" }));
}

it("keeps the dialog on a failed pre-check, then attaches an autolaunched custom session", async () => {
  const added = { id: "dev", kind: "custom", label: "Dev server", tmux_window: "dev-server" };
  let state: ReturnType<typeof mountApp> | null = null;
  state = mountApp((command, args) => {
    if (command !== "add_custom_session") throw new Error(`Unexpected command: ${command}`);
    const request = args.request as { name: string; revision: string; approved: boolean };
    if (request.name === "Database") {
      return { status: "pre_check_failed", name: "Database", pre_check: "docker info >/dev/null", output: "Cannot connect to the Docker daemon" };
    }
    state!.setCurrent(snapshot([added]));
    return {
      status: "added", label: "Dev server", autolaunch: true, message: "Added 'Dev server'",
      target: { project_id: "project", feature_id: "feature", session_id: "dev" },
    };
  });
  const picker = await openPicker();

  fireEvent.click(picker.getByRole("radio", { name: "Database" }));
  fireEvent.click(picker.getByRole("button", { name: "Create session" }));
  expect(await picker.findByText("Cannot connect to the Docker daemon")).toBeTruthy();
  expect(screen.getByRole("dialog", { name: "New session" })).toBeTruthy();

  fireEvent.click(picker.getByRole("radio", { name: "Dev server" }));
  fireEvent.click(picker.getByRole("button", { name: "Create session" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "New session" })).toBeNull());
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "add_custom_session").map(([, args]) => args))
    .toEqual([
      { request: { target: { project_id: "project", feature_id: "feature" }, name: "Database", revision: "rev-db", label: null, approved: false } },
      { request: { target: { project_id: "project", feature_id: "feature" }, name: "Dev server", revision: "rev-dev", label: null, approved: false } },
    ]);
  await act(async () => { await state!.client.invalidateQueries({ queryKey: ["workspace-snapshot"] }); });
  await waitFor(() => expect(screen.getByRole("tab", { name: /Dev server/ }).getAttribute("aria-selected")).toBe("true"));
  expect(screen.getByText("terminal attached")).toBeTruthy();
  state.client.clear();
});

it("opens VS Code into its own tab and reaches it again from the tree", async () => {
  let state: ReturnType<typeof mountApp> | null = null;
  state = mountApp((command, args) => {
    if (command !== "open_vscode") throw new Error(`Unexpected command: ${command}`);
    expect(args).toEqual({ target: { project_id: "project", feature_id: "feature" }, approved: false });
    state!.setCurrent(snapshot([], [{ id: "ed", name: "VS Code", state: "opening", closes_with_feature: true, started_at: "2026-10-07T10:00:00Z" }]));
    return { feature_id: "feature", workdir: "/w", editor_id: "ed", started_feature: false, message: "Opened VS Code in /w" };
  });
  const picker = await openPicker();
  fireEvent.click(picker.getByRole("radio", { name: "VS Code" }));
  fireEvent.click(picker.getByRole("button", { name: "Open VS Code" }));

  expect(await screen.findByText("Opened VS Code in /w")).toBeTruthy();
  await act(async () => { await state!.client.invalidateQueries({ queryKey: ["workspace-snapshot"] }); });
  await waitFor(() => expect(screen.getByRole("tab", { name: /VS Code/ }).getAttribute("aria-selected")).toBe("true"));
  expect(screen.getByText("Opening")).toBeTruthy();

  fireEvent.click(screen.getByRole("tab", { name: /Claude 1/ }));
  fireEvent.click(tree().getByRole("button", { name: /VS Code opening/ }));
  await waitFor(() => expect(screen.getByRole("tab", { name: /VS Code/ }).getAttribute("aria-selected")).toBe("true"));
  state.client.clear();
});
