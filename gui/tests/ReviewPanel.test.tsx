// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import ReviewPanel from "../src/ReviewPanel";
import type { ReviewView } from "../src/api";

afterEach(() => { cleanup(); vi.clearAllMocks(); });
const view: ReviewView = {
  workflow_id: "review", revision: 4, target: { project_id: "project", feature_id: "feature" },
  feature_name: "Feature", branch: "feature", base_ref: "main", selected_path: "code.rs",
  general_feedback: "Overall saved", has_prior_review: true, error: null, save_error: null,
  files: [{ diff: { path: "code.rs", old_path: null, status: "modified", additions: 1, deletions: 1, is_binary: false, patch: "",
    hunks: [{ header: "@@ -1,1 +1,1 @@", lines: [
      { kind: "removed", text: "-before", old_line: 1, new_line: null },
      { kind: "added", text: "+after", old_line: null, new_line: 1 },
    ] }] }, verdict: "rejected", feedback: "Needs work", severity: "blocker", changed_since_last: true,
    notes: "Developer explanation", comment: { text: "Saved question", severity: "question", resolved: false, carried: true },
    line_comments: [{ anchor: "line 1", text: "Kept thread", severity: "nit", resolved: false, draft: false, anchor_lost: true, suggestion: "suggested code" }] },
    { diff: { path: "image.bin", old_path: "old.bin", status: "renamed", additions: 0, deletions: 0, is_binary: true, hunks: [], patch: "rename from old.bin" },
      verdict: "approved", feedback: "", severity: "suggestion", comment: null, notes: null, changed_since_last: false, line_comments: [] }],
};
function mount(initial = view, onAct = vi.fn(async () => true)) {
  const props = { view: initial, busy: false, error: null, onAct };
  const component = render(<ReviewPanel {...props} />);
  return { onAct, update: (next: Partial<typeof props>) => component.rerender(<ReviewPanel {...props} {...next} />) };
}

it("renders verdicts, developer notes and carried threads alongside shared diff layouts", () => {
  const { update } = mount();
  expect(screen.getByText(/1 approved · 1 rejected · 0 undecided/)).toBeTruthy();
  expect(screen.getByText("Developer explanation")).toBeTruthy();
  expect(screen.getByText(/Saved question.*previous round/)).toBeTruthy();
  expect(screen.getByText(/Kept thread.*anchor lost/)).toBeTruthy();
  expect(screen.getByText("suggested code")).toBeTruthy();
  fireEvent.change(screen.getByRole("combobox", { name: "Review layout" }), { target: { value: "split" } });
  expect(within(screen.getByRole("table", { name: "Side-by-side hunk" })).getByText("+after")).toBeTruthy();
  update({ view: { ...view, selected_path: "image.bin" } });
  expect(screen.getByText("old.bin → image.bin")).toBeTruthy();
  expect(screen.getByText(/Binary file changed/)).toBeTruthy();
});

it("submits verdicts and navigation with stable file paths", async () => {
  const { onAct } = mount();
  fireEvent.click(screen.getByRole("button", { name: "Approve file" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "approve", path: "code.rs" }));
  fireEvent.click(screen.getByRole("button", { name: "Skip file" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "skip", path: "code.rs" }));
  fireEvent.click(screen.getByRole("button", { name: "Undo verdict" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "undo" }));
  fireEvent.click(screen.getByRole("button", { name: /image.bin approved/ }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "select", path: "image.bin" }));
});

it("edits file comments and severity and removes a comment through an explicit empty save", async () => {
  const { onAct } = mount();
  fireEvent.click(screen.getByRole("button", { name: "Edit file comment" }));
  fireEvent.change(screen.getByRole("textbox", { name: "File comment" }), { target: { value: "Updated question" } });
  fireEvent.change(screen.getByRole("combobox", { name: "Severity" }), { target: { value: "nit" } });
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Save comment" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "comment", path: "code.rs", text: "Updated question", severity: "nit" }));
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "File comment" })).toBeNull());
  fireEvent.click(screen.getByRole("button", { name: "Edit file comment" }));
  fireEvent.change(screen.getByRole("textbox", { name: "File comment" }), { target: { value: "" } });
  fireEvent.click(screen.getByRole("button", { name: "Save comment" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "comment", path: "code.rs", text: "", severity: "question" }));
});

it("edits rejection feedback and overall feedback without launching or sending a prompt", async () => {
  const { onAct } = mount();
  fireEvent.click(screen.getByRole("button", { name: "Reject file" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Rejection feedback" }), { target: { value: "Must fix" } });
  fireEvent.click(screen.getByRole("button", { name: "Save rejection" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "reject", path: "code.rs", feedback: "Must fix", severity: "blocker" }));
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "Rejection feedback" })).toBeNull());
  fireEvent.click(screen.getByRole("button", { name: "Overall feedback", exact: true }));
  fireEvent.change(screen.getByRole("textbox", { name: "Overall feedback draft" }), { target: { value: "Overall new" } });
  fireEvent.click(screen.getByRole("button", { name: "Save overall feedback" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "general", text: "Overall new" }));
});

it("defaults a fresh rejection to blocker, as the TUI does", async () => {
  const { onAct } = mount({ ...view, selected_path: "image.bin" });
  fireEvent.click(screen.getByRole("button", { name: "Reject file" }));
  expect((screen.getByRole("combobox", { name: "Severity" }) as HTMLSelectElement).value).toBe("blocker");
  fireEvent.change(screen.getByRole("textbox", { name: "Rejection feedback" }), { target: { value: "Wrong file" } });
  fireEvent.click(screen.getByRole("button", { name: "Save rejection" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "reject", path: "image.bin", feedback: "Wrong file", severity: "blocker" }));
});

it("closes without saving only after confirmation when a save failed", async () => {
  const { onAct } = mount({ ...view, save_error: "Read-only file system" });
  fireEvent.click(screen.getByRole("button", { name: "Close without saving" }));
  expect(screen.getByRole("alertdialog", { name: "Close review without saving" })).toBeTruthy();
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Close without saving" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard and close" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "discard" }));
});

it("preserves a failed edit and blocks duplicate actions while the command is pending", async () => {
  const onAct = vi.fn(async () => false);
  const { update } = mount(view, onAct);
  fireEvent.click(screen.getByRole("button", { name: "Edit file comment" }));
  fireEvent.change(screen.getByRole("textbox", { name: "File comment" }), { target: { value: "Keep draft" } });
  fireEvent.click(screen.getByRole("button", { name: "Save comment" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledOnce());
  expect((screen.getByRole("textbox", { name: "File comment" }) as HTMLTextAreaElement).value).toBe("Keep draft");
  update({ busy: true, error: "File changed; refresh first" });
  expect(screen.getByRole("alert").textContent).toContain("File changed");
  fireEvent.click(screen.getByRole("button", { name: "Save comment" }));
  fireEvent.click(screen.getByRole("button", { name: "Approve file" }));
  expect(onAct).toHaveBeenCalledOnce();
});

it("keeps unsaved drafts until discard is confirmed for pause and file navigation", async () => {
  const { onAct } = mount();
  fireEvent.click(screen.getByRole("button", { name: "Edit file comment" }));
  fireEvent.change(screen.getByRole("textbox", { name: "File comment" }), { target: { value: "Unsent" } });
  fireEvent.click(screen.getByRole("button", { name: "Pause review" }));
  expect(screen.getByRole("alertdialog", { name: "Discard unsaved review draft" })).toBeTruthy();
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  expect((screen.getByRole("textbox", { name: "File comment" }) as HTMLTextAreaElement).value).toBe("Unsent");
  fireEvent.click(screen.getByRole("button", { name: /image.bin approved/ }));
  fireEvent.click(screen.getByRole("button", { name: "Discard and continue" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "select", path: "image.bin" }));
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "File comment" })).toBeNull());
  fireEvent.click(screen.getByRole("button", { name: "Pause review" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "pause" }));
});

it("protects drafts on refresh and cancel, and reports save failures with a retry", async () => {
  const { onAct, update } = mount();
  fireEvent.click(screen.getByRole("button", { name: "Overall feedback", exact: true }));
  fireEvent.change(screen.getByRole("textbox", { name: "Overall feedback draft" }), { target: { value: "Draft" } });
  fireEvent.click(screen.getByRole("button", { name: "Refresh changes" }));
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  fireEvent.click(screen.getByRole("button", { name: "Cancel edit" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard and continue" }));
  expect(onAct).not.toHaveBeenCalled();
  update({ view: { ...view, save_error: "Disk full" } });
  expect(screen.getByRole("alert").textContent).toContain("Progress was not saved: Disk full");
  fireEvent.click(screen.getByRole("button", { name: "Retry save" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "retry_save" }));
  fireEvent.click(screen.getByRole("button", { name: "Reload saved review" }));
  expect(screen.getByRole("alertdialog")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Discard and continue" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "reload" }));
});

it("resolves and reopens saved file comments and handles an empty review", async () => {
  const { onAct, update } = mount();
  fireEvent.click(screen.getByRole("button", { name: "Resolve comment" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "toggle_resolved", path: "code.rs" }));
  update({ view: { ...view, files: [{ ...view.files[0], comment: { ...view.files[0].comment!, resolved: true } }] } });
  expect(screen.getByRole("button", { name: "Reopen comment" })).toBeTruthy();
  update({ view: { ...view, files: [], selected_path: null } });
  expect(screen.getByText("No changes to review.")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Approve file" })).toBeNull();
});
