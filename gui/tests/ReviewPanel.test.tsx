// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import ReviewPanel from "../src/ReviewPanel";
import type { ReviewView } from "../src/api";

afterEach(() => { cleanup(); vi.clearAllMocks(); });
const view: ReviewView = {
  workflow_id: "review", revision: 4, target: { project_id: "project", feature_id: "feature" },
  feature_name: "Feature", branch: "feature", base_ref: "main", selected_path: "code.rs",
  general_feedback: "Overall saved", has_prior_review: true, error: null, save_error: null, applied_suggestions: [],
  files: [{ diff: { path: "code.rs", old_path: null, status: "modified", additions: 1, deletions: 1, is_binary: false, patch: "",
    hunks: [{ header: "@@ -1,1 +1,1 @@", lines: [
      { kind: "removed", text: "-before", old_line: 1, new_line: null },
      { kind: "added", text: "+after", old_line: null, new_line: 1 },
    ] }] }, verdict: "rejected", feedback: "Needs work", severity: "blocker", changed_since_last: true,
    notes: "Developer explanation", comment: { text: "Saved question", severity: "question", resolved: false, carried: true },
    line_comments: [{ start: { old_line: null, new_line: 1 }, end: { old_line: null, new_line: 1 }, editable: false, anchor: "line 1", text: "Kept thread", severity: "nit", resolved: false, draft: false, anchor_lost: true, suggestion: "suggested code", apply_blocked: "anchor is no longer present in the current diff" }] },
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


const noThreadsView: ReviewView = { ...view, files: [{ ...view.files[0], line_comments: [] }, view.files[1]] };

function applicableView(): ReviewView {
  return { ...view, files: [{ ...view.files[0], diff: structuredClone(view.files[0].diff), line_comments: [{ ...view.files[0].line_comments[0], editable: true, anchor_lost: false, apply_blocked: null }] }] };
}

it("confirms a saved local replacement and can cancel without changing source", async () => {
  const { onAct } = mount(applicableView());
  fireEvent.click(screen.getByRole("button", { name: "Apply suggestion locally" }));
  const confirmation = screen.getByRole("alertdialog", { name: "Apply suggestion locally" });
  expect(confirmation.textContent).toContain("writes to your checkout");
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel application" }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Apply suggestion locally" }));
  fireEvent.click(screen.getByRole("button", { name: "Apply replacement" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "apply_suggestion", path: "code.rs",
    start: { old_line: null, new_line: 1 }, end: { old_line: null, new_line: 1 } }));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
});

it("keeps a refused application visible and blocks duplicate submissions during a write", async () => {
  const onAct = vi.fn(async () => false);
  const { update } = mount(applicableView(), onAct);
  fireEvent.click(screen.getByRole("button", { name: "Apply suggestion locally" }));
  fireEvent.click(screen.getByRole("button", { name: "Apply replacement" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledOnce());
  expect(screen.getByRole("alertdialog", { name: "Apply suggestion locally" })).toBeTruthy();
  expect(screen.getByText("suggested code")).toBeTruthy();
  update({ busy: true, error: "File changed; refresh before applying" });
  expect(screen.getByRole("alert").textContent).toContain("File changed");
  fireEvent.click(screen.getByRole("button", { name: "Apply replacement" }));
  fireEvent.click(screen.getByRole("button", { name: "Apply suggestion locally" }));
  expect(onAct).toHaveBeenCalledOnce();
  update({ error: "File changed; refresh before applying" });
  fireEvent.click(screen.getByRole("button", { name: "Cancel application" }));
  fireEvent.click(screen.getByRole("button", { name: "Refresh changes" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "refresh" }));
});

it("shows application blockers and protects unsaved editors and failed progress saves", () => {
  const { onAct, update } = mount();
  expect((screen.getByRole("button", { name: "Apply suggestion locally" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText(/Cannot apply locally: anchor is no longer/)).toBeTruthy();
  update({ view: applicableView() });
  fireEvent.click(screen.getByRole("button", { name: "Edit suggestion" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Suggested replacement" }), { target: { value: "Unsaved code" } });
  fireEvent.click(screen.getByRole("button", { name: "Apply suggestion locally" }));
  expect((screen.getByRole("button", { name: "Apply suggestion locally" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel edit" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard and continue" }));
  update({ view: { ...applicableView(), save_error: "Disk full" } });
  expect((screen.getByRole("button", { name: "Apply suggestion locally" }) as HTMLButtonElement).disabled).toBe(true);
});

it("shows persisted application history and the refreshed source without offering the consumed replacement", () => {
  const applied = applicableView();
  applied.applied_suggestions = ["code.rs:1"];
  applied.files[0].verdict = "undecided";
  applied.files[0].line_comments[0] = { ...applied.files[0].line_comments[0], suggestion: null, resolved: true };
  applied.files[0].diff.hunks[0].lines[1].text = "+suggested code";
  mount(applied);
  expect(screen.getByText("Applied locally (1)")).toBeTruthy();
  expect(screen.getByText("code.rs:1")).toBeTruthy();
  expect(screen.getByText(/does not undo source changes/)).toBeTruthy();
  expect(screen.getByText("+suggested code")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Apply suggestion locally" })).toBeNull();
});

it("selects a backwards range in unified diff order and submits its canonical base/current anchors", async () => {
  const { onAct } = mount(noThreadsView);
  fireEvent.click(screen.getByRole("button", { name: "Select line 1", exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "Select base line 1" }), { shiftKey: true });
  expect(screen.getByRole("button", { name: "Select base line 1" }).getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByRole("button", { name: "Select line 1", exact: true }).getAttribute("aria-pressed")).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "Comment on selection" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Line comment" }), { target: { value: "Range feedback" } });
  fireEvent.change(screen.getByRole("combobox", { name: "Severity" }), { target: { value: "blocker" } });
  fireEvent.click(screen.getByRole("button", { name: "Save comment" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "line_comment", path: "code.rs",
    start: { old_line: 1, new_line: null }, end: { old_line: null, new_line: 1 }, text: "Range feedback", severity: "blocker" }));
});

it("selects the two sides independently in split layout and seeds replacement code without diff markers", async () => {
  const { onAct } = mount(noThreadsView);
  fireEvent.change(screen.getByRole("combobox", { name: "Review layout" }), { target: { value: "split" } });
  fireEvent.click(screen.getByRole("button", { name: "Select line 1", exact: true }));
  expect(screen.getByRole("button", { name: "Select base line 1" }).getAttribute("aria-pressed")).toBe("false");
  fireEvent.click(screen.getByRole("button", { name: "Suggest replacement" }));
  expect((screen.getByRole("textbox", { name: "Suggested replacement" }) as HTMLTextAreaElement).value).toBe("after");
  fireEvent.change(screen.getByRole("textbox", { name: "Suggested replacement" }), { target: { value: "  replacement\n    next" } });
  fireEvent.click(screen.getByRole("button", { name: "Save suggestion" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "suggestion", path: "code.rs",
    start: { old_line: null, new_line: 1 }, end: { old_line: null, new_line: 1 }, text: "  replacement\n    next" }));
});

function editableView(): ReviewView {
  return { ...view, files: [{ ...view.files[0], line_comments: [{ ...view.files[0].line_comments[0], editable: true, anchor_lost: false,
    start: { old_line: 1, new_line: null }, end: { old_line: null, new_line: 1 }, anchor: "base line 1 – line 1" }] }] };
}

it("edits existing range prose and suggestions and clears the suggestion with an empty save", async () => {
  const { onAct } = mount(editableView());
  fireEvent.click(screen.getByRole("button", { name: "Edit line comment" }));
  expect((screen.getByRole("textbox", { name: "Line comment" }) as HTMLTextAreaElement).value).toBe("Kept thread");
  fireEvent.change(screen.getByRole("textbox", { name: "Line comment" }), { target: { value: "Edited thread" } });
  fireEvent.click(screen.getByRole("button", { name: "Save comment" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "line_comment", path: "code.rs",
    start: { old_line: 1, new_line: null }, end: { old_line: null, new_line: 1 }, text: "Edited thread", severity: "nit" }));
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "Line comment" })).toBeNull());
  fireEvent.click(screen.getByRole("button", { name: "Edit suggestion" }));
  expect((screen.getByRole("textbox", { name: "Suggested replacement" }) as HTMLTextAreaElement).value).toBe("suggested code");
  fireEvent.change(screen.getByRole("textbox", { name: "Suggested replacement" }), { target: { value: "" } });
  fireEvent.click(screen.getByRole("button", { name: "Save suggestion" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "suggestion", path: "code.rs",
    start: { old_line: 1, new_line: null }, end: { old_line: null, new_line: 1 }, text: "" }));
});

it("snaps a single selected line onto an existing range when editing its prose", async () => {
  const { onAct } = mount(editableView());
  fireEvent.click(screen.getByRole("button", { name: "Select line 1", exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "Comment on selection" }));
  fireEvent.click(screen.getByRole("button", { name: "Save comment" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "line_comment", path: "code.rs",
    start: { old_line: 1, new_line: null }, end: { old_line: null, new_line: 1 }, text: "Kept thread", severity: "nit" }));
});

it("highlights a saved thread's own span while its editor is open", () => {
  mount(editableView());
  fireEvent.click(screen.getByRole("button", { name: "Select base line 1" }));
  fireEvent.click(screen.getByRole("button", { name: "Edit line comment" }));
  expect(screen.getByRole("button", { name: "Select base line 1" }).getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByRole("button", { name: "Select line 1", exact: true }).getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByText("Selected base line 1 – line 1")).toBeTruthy();
});

it("refuses a selection that partly overlaps a saved thread instead of re-anchoring it", () => {
  const partial = editableView();
  partial.files[0].line_comments[0] = { ...partial.files[0].line_comments[0],
    start: { old_line: null, new_line: 1 }, end: { old_line: null, new_line: 1 }, anchor: "line 1" };
  const { onAct } = mount(partial);
  fireEvent.click(screen.getByRole("button", { name: "Select base line 1" }));
  fireEvent.click(screen.getByRole("button", { name: "Select line 1", exact: true }), { shiftKey: true });
  fireEvent.click(screen.getByRole("button", { name: "Suggest replacement" }));
  expect(screen.getByRole("alert").textContent).toContain("overlaps saved thread at line 1");
  expect(screen.queryByRole("textbox", { name: "Suggested replacement" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Select line 1", exact: true }));
  expect(screen.queryByRole("alert")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Suggest replacement" }));
  expect((screen.getByRole("textbox", { name: "Suggested replacement" }) as HTMLTextAreaElement).value).toBe("suggested code");
  expect(onAct).not.toHaveBeenCalled();
});

it("refuses to edit lines under a lost-anchor thread", () => {
  mount();
  fireEvent.click(screen.getByRole("button", { name: "Select line 1", exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "Comment on selection" }));
  expect(screen.getByRole("alert").textContent).toContain("anchor was lost");
  expect(screen.queryByRole("textbox", { name: "Line comment" })).toBeNull();
});

it("resolves range threads and refuses lost-anchor editing and AI-draft resolution", async () => {
  const { onAct, update } = mount(editableView());
  fireEvent.click(screen.getByRole("button", { name: "Resolve thread" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "toggle_line_resolved", path: "code.rs",
    start: { old_line: 1, new_line: null }, end: { old_line: null, new_line: 1 } }));
  const next = editableView(); next.files[0].line_comments[0].resolved = true;
  update({ view: next });
  expect(screen.getByRole("button", { name: "Reopen thread" })).toBeTruthy();
  next.files[0].line_comments[0].draft = true;
  update({ view: { ...next } });
  expect((screen.getByRole("button", { name: "Reopen thread" }) as HTMLButtonElement).disabled).toBe(true);
  update({ view });
  expect((screen.getByRole("button", { name: "Edit line comment" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "Edit suggestion" }) as HTMLButtonElement).disabled).toBe(true);
});

it("keeps failed suggestion drafts and blocks selection changes until edits are saved or discarded", async () => {
  const onAct = vi.fn(async () => false);
  const { update } = mount(editableView(), onAct);
  fireEvent.click(screen.getByRole("button", { name: "Edit suggestion" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Suggested replacement" }), { target: { value: "Keep code" } });
  expect((screen.getByRole("button", { name: "Select base line 1" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Save suggestion" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledOnce());
  expect((screen.getByRole("textbox", { name: "Suggested replacement" }) as HTMLTextAreaElement).value).toBe("Keep code");
  fireEvent.click(screen.getByRole("button", { name: "Refresh changes" }));
  expect(screen.getByRole("alertdialog", { name: "Discard unsaved review draft" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  update({ busy: true });
  fireEvent.click(screen.getByRole("button", { name: "Save suggestion" }));
  expect(onAct).toHaveBeenCalledOnce();
});
