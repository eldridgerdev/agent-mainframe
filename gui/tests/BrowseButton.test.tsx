// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { BrowseButton } from "../src/BrowseButton";
import CreateProjectForm from "../src/CreateProjectForm";
import type { HarnessInfo } from "../src/api";

vi.mock("../src/TerminalPane", () => ({ default: () => null }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const supported: HarnessInfo[] = [{ slug: "claude", display_name: "Claude Code" }];
const clients: QueryClient[] = [];

/** The native dialog is reachable only inside the Tauri runtime. */
function tauriRuntime(present: boolean) {
  const host = window as unknown as Record<string, unknown>;
  if (present) host.__TAURI_INTERNALS__ = {};
  else delete host.__TAURI_INTERNALS__;
}

beforeEach(() => tauriRuntime(true));
afterEach(() => {
  cleanup();
  clients.splice(0).forEach((client) => client.clear());
  vi.resetAllMocks();
  tauriRuntime(false);
});

it("picks a folder and hands the path over", async () => {
  vi.mocked(open).mockResolvedValue("/code/repo");
  const onPick = vi.fn();
  render(<BrowseButton kind="directory" label="Browse for repository folder" defaultPath=" /code " onPick={onPick} />);
  fireEvent.click(screen.getByRole("button", { name: "Browse for repository folder" }));
  await waitFor(() => expect(onPick).toHaveBeenCalledWith("/code/repo"));
  expect(open).toHaveBeenCalledWith({ directory: true, multiple: false, title: undefined, defaultPath: "/code" });
});

it("opens a file picker for kind=file", async () => {
  vi.mocked(open).mockResolvedValue("/docs/spec.md");
  const onPick = vi.fn();
  render(<BrowseButton kind="file" label="Browse for reference document" onPick={onPick} />);
  fireEvent.click(screen.getByRole("button", { name: "Browse for reference document" }));
  await waitFor(() => expect(onPick).toHaveBeenCalledWith("/docs/spec.md"));
  expect(vi.mocked(open).mock.calls[0][0]).toMatchObject({ directory: false, defaultPath: undefined });
});

it("a cancelled pick changes nothing", async () => {
  vi.mocked(open).mockResolvedValue(null);
  const onPick = vi.fn();
  render(<BrowseButton kind="directory" label="Browse" onPick={onPick} />);
  const button = screen.getByRole("button", { name: "Browse" });
  fireEvent.click(button);
  await waitFor(() => expect(open).toHaveBeenCalled());
  await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
  expect(onPick).not.toHaveBeenCalled();
  expect(screen.queryByRole("alert")).toBeNull();
});

it("is left out when the native dialog is unavailable", () => {
  tauriRuntime(false);
  render(<BrowseButton kind="directory" label="Browse" onPick={vi.fn()} />);
  expect(screen.queryByRole("button")).toBeNull();
});

it("says why when the picker fails, and can retry", async () => {
  vi.mocked(open).mockRejectedValueOnce("dialog.open not allowed").mockResolvedValueOnce("/code/repo");
  const onPick = vi.fn();
  render(<BrowseButton kind="directory" label="Browse" onPick={onPick} />);
  fireEvent.click(screen.getByRole("button", { name: "Browse" }));
  expect((await screen.findByRole("alert")).textContent)
    .toBe("The folder picker could not open (dialog.open not allowed). Type the path instead.");
  expect(onPick).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Browse" }));
  await waitFor(() => expect(onPick).toHaveBeenCalledWith("/code/repo"));
  expect(screen.queryByRole("alert")).toBeNull();
});

function mountProjectForm() {
  const requests: { path: string; project_name: string; dry_run: boolean }[] = [];
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "allowed_harnesses") return supported;
    if (command === "create_project") {
      const request = (args as { request: { path: string; project_name: string; dry_run: boolean } }).request;
      requests.push(request);
      return { ok: true, project_id: null, project_path: "/code/repo", is_git: true, message: "Dry run" };
    }
    return null;
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  clients.push(client);
  const onEdit = vi.fn();
  render(<QueryClientProvider client={client}>
    <CreateProjectForm agents={supported} pending={false} error={null} onEdit={onEdit} onCancel={vi.fn()} onSubmit={vi.fn()} />
  </QueryClientProvider>);
  return { requests, onEdit };
}

it("Add project: a picked folder fills the path and name and is checked like a blur", async () => {
  vi.mocked(open).mockResolvedValue("/code/repo");
  const { requests, onEdit } = mountProjectForm();
  fireEvent.click(screen.getByRole("button", { name: "Browse for repository folder" }));
  expect(await screen.findByText("Git repository found at /code/repo.")).toBeTruthy();
  expect((screen.getByLabelText(/^Repository path/) as HTMLInputElement).value).toBe("/code/repo");
  expect((screen.getByLabelText("Project name") as HTMLInputElement).value).toBe("repo");
  expect(requests).toEqual([{ path: "/code/repo", project_name: "repo", preferred_agent: null, dry_run: true }]);
  expect(onEdit).toHaveBeenCalled();
});

it("Add project: a cancelled pick leaves the typed path alone", async () => {
  vi.mocked(open).mockResolvedValue(null);
  const { requests, onEdit } = mountProjectForm();
  const path = screen.getByLabelText(/^Repository path/) as HTMLInputElement;
  fireEvent.change(path, { target: { value: "/typed" } });
  onEdit.mockClear();
  fireEvent.click(screen.getByRole("button", { name: "Browse for repository folder" }));
  await waitFor(() => expect(open).toHaveBeenCalledWith(expect.objectContaining({ defaultPath: "/typed" })));
  expect(path.value).toBe("/typed");
  expect(onEdit).not.toHaveBeenCalled();
  expect(requests).toEqual([]);
});
