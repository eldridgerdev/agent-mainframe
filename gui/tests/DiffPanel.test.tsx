// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import DiffPanel from "../src/DiffPanel";
import type { DiffView } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.clearAllMocks(); });
const target = { project_id: "project", feature_id: "feature" };
const view: DiffView = {
  target, feature_name: "Feature", branch: "feature", base_ref: "main", base_commit: "base123456",
  commit: null, commits: [{ hash: "commit123", short_hash: "commit1", subject: "Feature commit" }], commits_error: null,
  total_additions: 2, total_deletions: 1,
  files: [{ path: "code.rs", old_path: null, status: "modified", additions: 2, deletions: 1, is_binary: false, patch: "",
    hunks: [{ header: "@@ -1,2 +1,2 @@", lines: [
      { kind: "context", text: " context", old_line: 1, new_line: 1 },
      { kind: "removed", text: "-old 🦀", old_line: 2, new_line: null },
      { kind: "added", text: "+new 🦀", old_line: null, new_line: 2 },
    ] }, { header: "@@ -20,0 +21,1 @@", lines: [{ kind: "added", text: "+later", old_line: null, new_line: 21 }] }] },
    { path: "new.bin", old_path: "old.bin", status: "renamed", additions: 0, deletions: 0, is_binary: true, patch: "rename from old.bin", hunks: [] },
    { path: "mode.sh", old_path: null, status: "modified", additions: 0, deletions: 0, is_binary: false, patch: "old mode 100644\nnew mode 100755", hunks: [] }],
};
function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  const onClose = vi.fn();
  render(<QueryClientProvider client={client}><DiffPanel target={target} onClose={onClose} /></QueryClientProvider>);
  return onClose;
}

it("renders file and hunk navigation, split line numbers, renames, binaries and mode changes", async () => {
  vi.mocked(invoke).mockResolvedValue(view);
  mount();
  const first = (await screen.findAllByRole("table", { name: "Unified hunk" }))[0];
  expect(within(first).getByText("-old 🦀")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Next hunk" }));
  expect(screen.getByText("Hunk 2 of 2")).toBeTruthy();
  fireEvent.change(screen.getByRole("combobox", { name: "Layout" }), { target: { value: "split" } });
  const split = screen.getAllByRole("table", { name: "Side-by-side hunk" })[0];
  const row = within(split).getByText("-old 🦀").closest("tr")!;
  expect(within(row).getByText("+new 🦀")).toBeTruthy();
  expect(within(row).getAllByText("2")).toHaveLength(2);
  fireEvent.click(screen.getByRole("button", { name: /new.bin renamed/ }));
  expect(screen.getByText("old.bin → new.bin")).toBeTruthy();
  expect(screen.getByText(/Binary file changed/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /mode.sh modified/ }));
  expect(screen.getByText(/old mode 100644/)).toBeTruthy();
  fireEvent.change(screen.getByRole("textbox", { name: "Filter files" }), { target: { value: "missing" } });
  expect(screen.getByText("No files match your filter.")).toBeTruthy();
  expect(vi.mocked(invoke)).toHaveBeenCalledTimes(1);
});

it("loads scope, whitespace, context and base changes with explicit requests and refreshes", async () => {
  vi.mocked(invoke).mockResolvedValue(view);
  mount();
  await screen.findByText("+new 🦀");
  fireEvent.change(screen.getByRole("combobox", { name: "Diff scope" }), { target: { value: "commit123" } });
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenLastCalledWith("load_diff", {
    target, options: { commit: "commit123", base_ref: null, ignore_whitespace: false, context: "standard" },
  }));
  expect((screen.getByRole("textbox", { name: "Base ref" }) as HTMLInputElement).disabled).toBe(true);
  fireEvent.change(screen.getByRole("combobox", { name: "Diff scope" }), { target: { value: "" } });
  await screen.findByText("+new 🦀");
  fireEvent.click(screen.getByRole("checkbox", { name: "Ignore whitespace" }));
  await screen.findByText("+new 🦀");
  fireEvent.change(screen.getByRole("combobox", { name: "Context" }), { target: { value: "full" } });
  await screen.findByText("+new 🦀");
  fireEvent.change(screen.getByRole("textbox", { name: "Base ref" }), { target: { value: "release" } });
  fireEvent.click(screen.getByRole("button", { name: "Apply base" }));
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenLastCalledWith("load_diff", {
    target, options: { commit: null, base_ref: "release", ignore_whitespace: true, context: "full" },
  }));
  await screen.findByText("+new 🦀");
  const calls = vi.mocked(invoke).mock.calls.length;
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledTimes(calls + 1));
});

it("does not display an old scope's delayed response after switching scope", async () => {
  let resolveOld!: (value: DiffView) => void;
  vi.mocked(invoke).mockResolvedValueOnce(view)
    .mockImplementationOnce(() => new Promise((resolve) => { resolveOld = resolve; }))
    .mockResolvedValue({ ...view, files: [], total_additions: 0, total_deletions: 0 });
  mount();
  await screen.findByText("+new 🦀");
  fireEvent.click(screen.getByRole("checkbox", { name: "Ignore whitespace" }));
  await waitFor(() => expect(resolveOld).toBeTypeOf("function"));
  fireEvent.change(screen.getByRole("combobox", { name: "Context" }), { target: { value: "full" } });
  await screen.findByText("No changes in this scope.");
  resolveOld(view);
  await waitFor(() => expect(screen.queryByText("+new 🦀")).toBeNull());
  expect(screen.getByText("No changes in this scope.")).toBeTruthy();
});

it("reports deleted targets and retries without displaying an old successful snapshot", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(view).mockRejectedValueOnce({ kind: "not_found", message: "Feature was deleted; refresh and retry" }).mockResolvedValue(view);
  const onClose = mount();
  await screen.findByText("+new 🦀");
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await screen.findByRole("alert");
  expect(screen.queryByText("+new 🦀")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await screen.findByText("+new 🦀");
  fireEvent.click(screen.getAllByRole("button", { name: "Close", exact: true })[0]);
  expect(onClose).toHaveBeenCalledOnce();
});

it("keeps no-newline markers once in split view and leaves unmatched additions on the new side", async () => {
  vi.mocked(invoke).mockResolvedValue({ ...view, files: [{ ...view.files[0], hunks: [{ header: "@@ -1,1 +1,2 @@", lines: [
    { kind: "removed", text: "-before", old_line: 1, new_line: null },
    { kind: "added", text: "+after", old_line: null, new_line: 1 },
    { kind: "added", text: "+extra", old_line: null, new_line: 2 },
    { kind: "marker", text: "\\ No newline at end of file", old_line: null, new_line: null },
  ] }] }] });
  mount();
  await screen.findByText("+extra");
  fireEvent.change(screen.getByRole("combobox", { name: "Layout" }), { target: { value: "split" } });
  const extra = screen.getByText("+extra").closest("tr")!;
  expect(extra.querySelectorAll("td")[0].textContent).toBe("");
  expect(extra.querySelectorAll("td")[2].textContent).toBe("2");
  expect(screen.getAllByText("\\ No newline at end of file")).toHaveLength(1);
  expect(screen.getByText("\\ No newline at end of file").closest("td")!.colSpan).toBe(4);
});
