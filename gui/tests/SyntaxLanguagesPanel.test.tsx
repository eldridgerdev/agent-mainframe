// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import SyntaxLanguagesPanel from "../src/SyntaxLanguagesPanel";
import type { SyntaxInstallView, SyntaxLanguagesView } from "../src/syntaxApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.resetAllMocks(); vi.useRealTimers(); });
const idle: SyntaxInstallView = { operation_id: 0, action: null, language: null, language_key: null, running: false, output: null, message: null, error: null, completed: 0 };
const catalog: SyntaxLanguagesView = { operation: idle, languages: [
  { key: "json", title: "JSON", state: "available", revision: "json-v1" },
  { key: "rust", title: "Rust", state: "installed", revision: "rust-v1" },
  { key: "python", title: "Python", state: "broken", revision: "python-v1" },
] };
function click(name: string) { fireEvent.click(screen.getByRole("button", { name })); }

it("lists the catalog and stages every action without mutating until confirmation", async () => {
  vi.mocked(invoke).mockResolvedValue(catalog);
  const close = vi.fn(); render(<SyntaxLanguagesPanel onClose={close} />);
  await screen.findByRole("button", { name: "Install JSON parser" });
  expect(screen.getByText("Needs repair")).toBeTruthy();
  for (const [action, language] of [["Install", "JSON"], ["Repair", "Python"], ["Uninstall", "Rust"]]) {
    click(`${action} ${language} parser`);
    expect(screen.getByRole("alertdialog", { name: `${action} ${language} parser` })).toBeTruthy();
    click("Cancel");
  }
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(close).not.toHaveBeenCalled();
  click("Uninstall Rust parser"); click("Close manager");
  expect(screen.queryByRole("alertdialog")).toBeNull(); expect(close).not.toHaveBeenCalled();
  click("Close manager"); expect(close).toHaveBeenCalledOnce();
});

it.each([["Install", "JSON", "json", "install", "json-v1"], ["Repair", "Python", "python", "repair", "python-v1"], ["Uninstall", "Rust", "rust", "uninstall", "rust-v1"]])(
  "binds explicit %s confirmation to the listed disk revision", async (title, label, language, action, revision) => {
    let resolve!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementation(async command => {
      if (command === "syntax_languages") return catalog;
      return new Promise(done => { resolve = done; });
    });
    const close = vi.fn(); render(<SyntaxLanguagesPanel onClose={close} />);
    await screen.findByRole("button", { name: `${title} ${label} parser` });
    click(`${title} ${label} parser`); click(`${title} parser`); click(`${title} parser`);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("syntax_languages_apply", { request: { language, action, revision } }));
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("button", { name: "Close manager" })).toHaveProperty("disabled", true);
    expect(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Cancel" })).toHaveProperty("disabled", true);
    await act(async () => resolve({ ...idle, operation_id: 1, running: true, language: label, language_key: language, action }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Refresh languages" })).toHaveProperty("disabled", true);
    click("Close manager"); expect(close).toHaveBeenCalledOnce();
  });

it("reports stale refusal, preserves confirmation, and refreshes before retry", async () => {
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "syntax_languages") return catalog;
    throw { kind: "conflict", message: "Parser files changed. Refresh languages and confirm again." };
  });
  render(<SyntaxLanguagesPanel onClose={vi.fn()} />);
  await screen.findByRole("button", { name: "Uninstall Rust parser" });
  click("Uninstall Rust parser"); click("Uninstall parser");
  expect((await screen.findByRole("alert")).textContent).toContain("Parser files changed");
  expect(screen.getByRole("alertdialog")).toBeTruthy();
  click("Refresh languages");
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(invoke).toHaveBeenCalledTimes(3);
});

it("reopens a running shared worker and reloads external state after completion", async () => {
  let status: SyntaxInstallView = { ...idle, operation_id: 8, action: "repair", language: "JSON", language_key: "json", running: true, output: "compiling parser", completed: 7 };
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "syntax_install_status") return status;
    if (command === "syntax_languages") return status.running ? { operation: status, languages: [] } : { ...catalog, operation: status };
    throw new Error(`unexpected ${command}`);
  });
  const first = render(<SyntaxLanguagesPanel onClose={vi.fn()} />);
  await screen.findByText("compiling parser"); first.unmount();
  render(<SyntaxLanguagesPanel onClose={vi.fn()} />);
  await screen.findByText("compiling parser");
  expect(screen.queryByRole("button", { name: "Install JSON parser" })).toBeNull();
  status = { ...status, running: false, completed: 8, message: "Repaired JSON parser" };
  expect(await screen.findByRole("button", { name: "Install JSON parser" }, { timeout: 2500 })).toBeTruthy();
  expect(screen.getByText("Repaired JSON parser")).toBeTruthy();
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "syntax_languages_apply")).toHaveLength(0);
});

it("does not apply late progress from a closed manager to its replacement", async () => {
  let resolveOld!: (value: unknown) => void;
  let generation = 1;
  const running = { ...idle, operation_id: 1, running: true, language: "JSON", completed: 0 };
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "syntax_languages") return { ...catalog, operation: generation === 1 ? running : { ...idle, operation_id: 2, completed: 2, message: "new manager state" } };
    if (command === "syntax_install_status") return new Promise(done => { resolveOld = done; });
    throw new Error(`unexpected ${command}`);
  });
  const first = render(<SyntaxLanguagesPanel onClose={vi.fn()} />);
  await waitFor(() => expect(resolveOld).toBeTypeOf("function"), { timeout: 2500 });
  first.unmount(); generation = 2;
  render(<SyntaxLanguagesPanel onClose={vi.fn()} />);
  await screen.findByText("new manager state");
  await act(async () => resolveOld({ ...running, running: false, completed: 1, message: "stale reply" }));
  expect(screen.queryByText("stale reply")).toBeNull();
  expect(screen.getByText("new manager state")).toBeTruthy();
});

it("reports unreadable parser state and retries without any mutation", async () => {
  vi.mocked(invoke).mockRejectedValueOnce({ kind: "io", message: "Cannot read parser directory" }).mockResolvedValueOnce(catalog);
  render(<SyntaxLanguagesPanel onClose={vi.fn()} />);
  expect((await screen.findByRole("alert")).textContent).toContain("Cannot read parser directory");
  expect(screen.queryByRole("button", { name: "Install JSON parser" })).toBeNull();
  click("Refresh languages");
  await screen.findByRole("button", { name: "Install JSON parser" });
  expect(screen.queryByRole("alert")).toBeNull();
  expect(vi.mocked(invoke).mock.calls.every(([command]) => command === "syntax_languages")).toBe(true);
});

it("adopts a per-file worker that starts while the catalog request is pending", async () => {
  let resolveList!: (value: unknown) => void;
  vi.mocked(invoke).mockImplementation(() => new Promise(done => { resolveList = done; }));
  render(<SyntaxLanguagesPanel onClose={vi.fn()} />);
  const running = { ...idle, operation_id: 4, running: true, language: "JSON", language_key: "json", output: "per-file worker", completed: 3 };
  act(() => window.dispatchEvent(new CustomEvent("amf:syntax-operation-started", { detail: running })));
  expect(screen.getByText("per-file worker")).toBeTruthy();
  await act(async () => resolveList(catalog));
  expect(screen.getByText("per-file worker")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Install JSON parser" })).toBeNull();
  expect(screen.getByRole("button", { name: "Refresh languages" })).toHaveProperty("disabled", true);
});
