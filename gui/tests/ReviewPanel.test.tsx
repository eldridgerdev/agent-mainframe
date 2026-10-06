// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import ReviewPanel from "../src/ReviewPanel";
import type { ReviewView } from "../src/api";

afterEach(() => { cleanup(); vi.clearAllMocks(); });
const view: ReviewView = {
  workflow_id: "review", revision: 4, target: { project_id: "project", feature_id: "feature" },
  feature_name: "Feature", branch: "feature", base_ref: "main", selected_path: "code.rs",
  general_feedback: "Overall saved", has_prior_review: true, error: null, save_error: null, applied_suggestions: [], history: null, summary: null, check_command: null, check: null, finish: null,
  ai: { precall: null, running: false, walkthrough_path: null, co_review_path: null, overview_running: false, overview: null, question_running: false, questions: [], question_error: null, comment_draft: null, ready_comment: null, harnesses: ["claude", "codex", "opencode", "pi"], message: null },
  files: [{ diff: { path: "code.rs", old_path: null, status: "modified", additions: 1, deletions: 1, is_binary: false, patch: "",
    hunks: [{ header: "@@ -1,1 +1,1 @@", lines: [
      { kind: "removed", text: "-before", old_line: 1, new_line: null },
      { kind: "added", text: "+after", old_line: null, new_line: 1 },
    ] }] }, verdict: "rejected", feedback: "Needs work", severity: "blocker", changed_since_last: true,
    notes: "Developer explanation", walkthrough: null, comment: { text: "Saved question", severity: "question", resolved: false, carried: true },
    line_comments: [{ start: { old_line: null, new_line: 1 }, end: { old_line: null, new_line: 1 }, editable: false, anchor: "line 1", text: "Kept thread", severity: "nit", resolved: false, draft: false, anchor_lost: true, suggestion: "suggested code", apply_blocked: "anchor is no longer present in the current diff" }] },
    { diff: { path: "image.bin", old_path: "old.bin", status: "renamed", additions: 0, deletions: 0, is_binary: true, hunks: [], patch: "rename from old.bin" },
      verdict: "approved", feedback: "", severity: "suggestion", comment: null, notes: null, walkthrough: null, changed_since_last: false, line_comments: [] }],
};
function mount(initial = view, onAct = vi.fn(async () => true)) {
  const props = { view: initial, busy: false, error: null, onAct };
  const component = render(<ReviewPanel {...props} />);
  return { onAct, update: (next: Partial<typeof props>) => component.rerender(<ReviewPanel {...props} {...next} />) };
}

const history: NonNullable<ReviewView["history"]> = {
  selected: 0, current_unresolved: 1, archive_available: true, archive_loaded: false, error: null,
  rounds: [{ title: "Review — yesterday", carried_unresolved: 2 }],
  markdown: "## Current Review\n\nLocal feedback",
};

const summary: NonNullable<ReviewView["summary"]> = {
  undecided: 1, pending_suggestions: 2, failures: [],
  rows: [
    { path: "code.rs", title: "code.rs — needs work", text: "Needs work", severity: "blocker", suggestion: null, apply_blocked: null },
    { path: "code.rs", title: "code.rs:1", text: "Kept thread", severity: "nit", suggestion: "suggested code", apply_blocked: "lost anchor" },
    { path: "image.bin", title: "image.bin — no verdict", text: "", severity: null, suggestion: null, apply_blocked: null },
    { path: null, title: "Overall feedback", text: "Overall saved", severity: null, suggestion: null, apply_blocked: null },
  ],
};

it("opens the complete pre-finish summary without discarding a draft or its file filter", async () => {
  const { onAct, update } = mount();
  fireEvent.change(screen.getByRole("textbox", { name: "Filter review files" }), { target: { value: "code.rs" } });
  fireEvent.click(screen.getByRole("button", { name: "Edit file comment" }));
  fireEvent.change(screen.getByRole("textbox", { name: "File comment" }), { target: { value: "Unsent multiline\ndraft" } });
  fireEvent.click(screen.getByRole("button", { name: "Pre-finish summary" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "summary_open" }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  update({ view: { ...view, summary } });
  expect(screen.getByRole("heading", { name: "image.bin — no verdict" })).toBeTruthy();
  expect(screen.getByText(/1 file\(s\) have no verdict/)).toBeTruthy();
  expect(screen.getByText("suggested code")).toBeTruthy();
  expect(screen.getByText(/Cannot apply locally: lost anchor/)).toBeTruthy();
  expect((screen.getByRole("button", { name: "Apply pending suggestions" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Return to review" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "summary_close" }));
  update({ view });
  expect((screen.getByRole("textbox", { name: "File comment" }) as HTMLTextAreaElement).value).toBe("Unsent multiline\ndraft");
  expect((screen.getByRole("textbox", { name: "Filter review files" }) as HTMLInputElement).value).toBe("code.rs");
});

it("retains a question draft through the summary and closes it with Escape without pausing", async () => {
  const { onAct, update } = mount();
  fireEvent.click(screen.getByRole("button", { name: "Ask about file" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Review question" }), { target: { value: "Question draft" } });
  update({ view: { ...view, summary } });
  fireEvent.keyDown(window, { key: "Escape" });
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "summary_close" }));
  update({ view });
  expect((screen.getByRole("textbox", { name: "Review question" }) as HTMLTextAreaElement).value).toBe("Question draft");
  expect(onAct).not.toHaveBeenCalledWith({ kind: "pause" });
});

it("requires explicit batch confirmation and cancelling starts no action", () => {
  const { onAct } = mount({ ...view, summary });
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Apply pending suggestions" }));
  expect(screen.getByRole("alertdialog", { name: "Apply pending suggestions locally" }).textContent).toContain("writes source files");
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel batch application" }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(onAct).not.toHaveBeenCalled();
});

it("sends exactly one batch action, retains failed confirmation for retry and reflects partial results", async () => {
  let resolve!: (value: boolean) => void;
  const onAct = vi.fn(() => new Promise<boolean>((done) => { resolve = done; }));
  const { update } = mount({ ...view, summary }, onAct);
  fireEvent.click(screen.getByRole("button", { name: "Apply pending suggestions" }));
  fireEvent.click(screen.getByRole("button", { name: "Apply batch locally" }));
  fireEvent.click(screen.getByRole("button", { name: "Apply batch locally" }));
  expect(onAct).toHaveBeenCalledTimes(1);
  expect(onAct).toHaveBeenCalledWith({ kind: "apply_finish_suggestions" });
  resolve(false);
  await waitFor(() => expect((screen.getByRole("button", { name: "Apply batch locally" }) as HTMLButtonElement).disabled).toBe(false));
  expect(screen.getByRole("alertdialog")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Apply batch locally" }));
  resolve(true);
  update({ view: { ...view, revision: 5, applied_suggestions: ["code.rs:1"], summary: { ...summary, pending_suggestions: 1, failures: ["other.rs:9: overlapping replacements"] } } });
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(screen.getByText("other.rs:9: overlapping replacements")).toBeTruthy();
  expect(screen.getByText("1 open suggestion(s) · 1 applied locally")).toBeTruthy();
});

it("blocks batch writes during AI work or save errors and retains the summary for save retry", async () => {
  const { onAct, update } = mount({ ...view, summary, ai: { ...view.ai, running: true } });
  expect((screen.getByRole("button", { name: "Apply pending suggestions" }) as HTMLButtonElement).disabled).toBe(true);
  update({ view: { ...view, summary, save_error: "disk full" } });
  expect(screen.getByRole("alert").textContent).toContain("disk full");
  expect((screen.getByRole("button", { name: "Apply pending suggestions" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Retry save" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "retry_save" }));
  update({ view: { ...view, summary } });
  expect((screen.getByRole("button", { name: "Apply pending suggestions" }) as HTMLButtonElement).disabled).toBe(false);
  update({ view: { ...view, summary: { ...summary, pending_suggestions: 0 } } });
  expect((screen.getByRole("button", { name: "Apply pending suggestions" }) as HTMLButtonElement).disabled).toBe(true);
});

it("requires a fresh confirmation when the review revision changes and prevents busy submissions", () => {
  const { onAct, update } = mount({ ...view, summary });
  fireEvent.click(screen.getByRole("button", { name: "Apply pending suggestions" }));
  update({ view: { ...view, revision: 5, summary } });
  expect(screen.queryByRole("alertdialog")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Apply pending suggestions" }));
  update({ view: { ...view, revision: 5, summary }, busy: true });
  fireEvent.click(screen.getByRole("button", { name: "Apply batch locally" }));
  expect(onAct).not.toHaveBeenCalled();
});

it("browses completed rounds and explicitly loads the archive without editing", async () => {
  const { onAct, update } = mount({ ...view, history });
  expect(screen.getByRole("heading", { name: "Current Review" })).toBeTruthy();
  expect(screen.getByText("1 open threads")).toBeTruthy();
  expect(screen.getByText("2 carried unresolved")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Approve file" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /Review — yesterday/ }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "history_select", round: 1 }));
  update({ view: { ...view, history: { ...history, selected: 1, markdown: "## Completed round\n\n**Agent:** Fixed it\n\n```suggestion\nreplacement\n```" } } });
  expect(screen.getByText("replacement")).toBeTruthy();
  expect(screen.getByText(/Fixed it/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Load older rounds" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "history_load_older" }));
  update({ view: { ...view, history: { ...history, archive_loaded: true } } });
  expect(screen.queryByRole("button", { name: "Load older rounds" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /Current 1 open threads/ }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "history_select", round: 0 }));
});

it("retains a dirty editor through history navigation and returns without pausing", async () => {
  const { onAct, update } = mount();
  fireEvent.click(screen.getByRole("button", { name: "Edit file comment" }));
  fireEvent.change(screen.getByRole("textbox", { name: "File comment" }), { target: { value: "Keep my unsaved draft" } });
  fireEvent.click(screen.getByRole("button", { name: "Review history" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "history_open" }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  update({ view: { ...view, history } });
  fireEvent.click(screen.getByRole("button", { name: "Return to review" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "history_close" }));
  update({ view });
  expect((screen.getByRole("textbox", { name: "File comment" }) as HTMLTextAreaElement).value).toBe("Keep my unsaved draft");
  expect(onAct).not.toHaveBeenCalledWith({ kind: "pause" });
  expect(onAct).not.toHaveBeenCalledWith({ kind: "retry_save" });
});

it("retains question text when history is closed with Escape", async () => {
  const { onAct, update } = mount();
  fireEvent.click(screen.getByRole("button", { name: "Ask about file" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Review question" }), { target: { value: "Why this change?" } });
  fireEvent.click(screen.getByRole("button", { name: "Review history" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "history_open" }));
  update({ view: { ...view, history } });
  fireEvent.keyDown(window, { key: "Escape" });
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "history_close" }));
  update({ view });
  expect((screen.getByRole("textbox", { name: "Review question" }) as HTMLTextAreaElement).value).toBe("Why this change?");
  expect(onAct).not.toHaveBeenCalledWith({ kind: "cancel_ai" });
});

it("retains edited AI comment drafts through history navigation", async () => {
  const { onAct, update } = mount(withDraft);
  fireEvent.change(screen.getByRole("textbox", { name: "AI comment draft" }), { target: { value: "Edited AI prose" } });
  fireEvent.click(screen.getByRole("button", { name: "Review history" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "history_open" }));
  update({ view: { ...withDraft, history } });
  fireEvent.click(screen.getByRole("button", { name: "Return to review" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "history_close" }));
  update({ view: withDraft });
  expect((screen.getByRole("textbox", { name: "AI comment draft" }) as HTMLTextAreaElement).value).toBe("Edited AI prose");
  expect(onAct).not.toHaveBeenCalledWith({ kind: "discard_question_draft" });
});

it("shows history read failures and empty rounds and prevents submissions while busy", () => {
  const { onAct, update } = mount({ ...view, history: { ...history, rounds: [], error: "Could not read review history" } });
  expect(screen.getByRole("alert").textContent).toContain("Could not read review history");
  expect(screen.getByText("No completed rounds loaded.")).toBeTruthy();
  update({ busy: true });
  fireEvent.click(screen.getByRole("button", { name: "Load older rounds" }));
  fireEvent.click(screen.getByRole("button", { name: "Return to review" }));
  fireEvent.keyDown(window, { key: "Escape" });
  expect(onAct).not.toHaveBeenCalled();
});

it("preserves drafts after a failed history request and waits for pre-call decisions", async () => {
  const { onAct, update } = mount(view, vi.fn(async () => false));
  fireEvent.click(screen.getByRole("button", { name: "Overall feedback", exact: true }));
  fireEvent.change(screen.getByRole("textbox", { name: "Overall feedback draft" }), { target: { value: "Unsaved overall" } });
  fireEvent.click(screen.getByRole("button", { name: "Review history" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "history_open" }));
  expect((screen.getByRole("textbox", { name: "Overall feedback draft" }) as HTMLTextAreaElement).value).toBe("Unsaved overall");
  update({ view: { ...view, ai: { ...view.ai, precall: { title: "Question", harness: "codex", viewing: false, preview: "prompt" } } } });
  fireEvent.click(screen.getByRole("button", { name: "Review history" }));
  expect(onAct).toHaveBeenCalledTimes(1);
});

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

it("starts walkthroughs, changeset overview and co-review only on explicit clicks", async () => {
  const { onAct } = mount({ ...view, files: view.files.map((f) => ({ ...f, notes: null })) });
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Generate walkthrough" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "walkthrough", path: "code.rs" }));
  fireEvent.click(screen.getByRole("button", { name: "AI co-review file" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "co_review", path: "code.rs" }));
  fireEvent.click(screen.getByRole("button", { name: "Changeset overview", exact: true }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "overview" }));
});

it("offers preview, continue and cancel while a pre-call notice keeps the review visible", async () => {
  const { onAct, update } = mount({ ...view, ai: { ...view.ai, precall: { title: "Walkthrough", harness: "Claude", preview: "Rendered prompt", viewing: false } } });
  expect(screen.getByText("Developer explanation")).toBeTruthy();
  expect((screen.getByRole("button", { name: "Approve file" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "View prompt" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "precall_toggle_view" }));
  update({ view: { ...view, ai: { ...view.ai, precall: { title: "Walkthrough", harness: "Claude", preview: "Rendered prompt", viewing: true } } } });
  expect(screen.getByText("Rendered prompt")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Continue AI call" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "precall_confirm" }));
  fireEvent.click(screen.getByRole("button", { name: "Cancel AI call" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "precall_cancel" }));
});

it("keeps a multiline question through pre-call cancellation, failed commands and polling", async () => {
  const { onAct, update } = mount(view, vi.fn(async () => false));
  fireEvent.click(screen.getByRole("button", { name: "Ask about file" }));
  const textbox = screen.getByRole("textbox", { name: "Review question" });
  fireEvent.change(textbox, { target: { value: "Why?\nCheck the helper 🦀" } });
  fireEvent.change(screen.getByRole("combobox", { name: "Answering harness" }), { target: { value: "codex" } });
  fireEvent.click(screen.getByRole("button", { name: "Ask review question" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "ask", path: "code.rs", start: null, end: null, question: "Why?\nCheck the helper 🦀", harness: "codex" }));
  update({ view: { ...view, revision: view.revision + 1, ai: { ...view.ai, precall: { title: "Question", harness: "Codex", preview: "preview", viewing: false } } } });
  expect((textbox as HTMLTextAreaElement).value).toBe("Why?\nCheck the helper 🦀");
  fireEvent.click(screen.getByRole("button", { name: "Cancel AI call" }));
  update({ view: { ...view, revision: view.revision + 2 } });
  expect((textbox as HTMLTextAreaElement).value).toBe("Why?\nCheck the helper 🦀");
  fireEvent.click(screen.getByRole("button", { name: "Pause review" }));
  expect(screen.getByRole("alertdialog", { name: "Discard unsaved review draft" })).toBeTruthy();
});

it.each(["claude", "codex", "opencode", "pi"])("asks selected source coordinates with %s and shows returned answers", async (harness) => {
  const { onAct, update } = mount(noThreadsView);
  fireEvent.click(screen.getAllByRole("button", { name: "Select line 1" })[0]);
  fireEvent.click(screen.getByRole("button", { name: "Ask about selection" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Review question" }), { target: { value: "Why this line?" } });
  fireEvent.change(screen.getByRole("combobox", { name: "Answering harness" }), { target: { value: harness } });
  fireEvent.click(screen.getByRole("button", { name: "Ask review question" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "ask", path: "code.rs", start: { old_line: null, new_line: 1 }, end: { old_line: null, new_line: 1 }, question: "Why this line?", harness }));
  update({ view: { ...noThreadsView, revision: 5, ai: { ...view.ai, questions: [{ question: "Why this line?", answer: "An explanation", error: null, focus: "code.rs, new line 1", path: "code.rs", start: { old_line: null, new_line: 1 }, end: { old_line: null, new_line: 1 }, harness }] } } });
  expect(screen.getByText("An explanation")).toBeTruthy();
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "Review question" })).toBeNull());
});

it("requires explicit acceptance of co-review drafts and supports dismissal", async () => {
  const next = editableView();
  next.files[0].line_comments[0].draft = true;
  const { onAct } = mount(next);
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Accept AI draft" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "accept_draft", path: "code.rs", start: next.files[0].line_comments[0].start, end: next.files[0].line_comments[0].end }));
  fireEvent.click(screen.getByRole("button", { name: "Dismiss AI draft" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "dismiss_draft", path: "code.rs", start: next.files[0].line_comments[0].start, end: next.files[0].line_comments[0].end }));
});

it("shows in-flight progress, blocks a second AI run, and confirms cancellation on pause", async () => {
  const { onAct } = mount({ ...view, ai: { ...view.ai, running: true, co_review_path: "code.rs" } });
  expect(screen.getByText(/Co-reviewing code.rs/)).toBeTruthy();
  expect((screen.getByRole("button", { name: "AI co-review file" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Pause review" }));
  expect(screen.getByText(/cancel the running AI request/)).toBeTruthy();
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  fireEvent.click(screen.getByRole("button", { name: "Cancel AI request" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "cancel_ai" }));
});

it("retains submitted question text while running and after a failed answer", async () => {
  const { update } = mount(noThreadsView);
  fireEvent.click(screen.getByRole("button", { name: "Ask about file" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Review question" }), { target: { value: "Keep this question" } });
  fireEvent.click(screen.getByRole("button", { name: "Ask review question" }));
  const turn = { question: "Keep this question", answer: null, error: null, focus: "code.rs", path: "code.rs", start: null, end: null, harness: "claude" as const };
  update({ view: { ...noThreadsView, ai: { ...view.ai, running: true, question_running: true, questions: [turn] } } });
  expect((screen.getByRole("textbox", { name: "Review question" }) as HTMLTextAreaElement).value).toBe("Keep this question");
  update({ view: { ...noThreadsView, revision: 5, ai: { ...view.ai, questions: [{ ...turn, error: "Harness failed" }], question_error: "Harness failed" } } });
  expect((screen.getByRole("textbox", { name: "Review question" }) as HTMLTextAreaElement).value).toBe("Keep this question");
  expect((screen.getByRole("button", { name: "Ask review question" }) as HTMLButtonElement).disabled).toBe(false);
});

it("retries a failed question about its own file, lines and harness, not the current selection", async () => {
  const failed = { question: "Why this line?", answer: null, error: "Harness failed", focus: "code.rs, new line 1",
    path: "code.rs", start: { old_line: null, new_line: 1 }, end: { old_line: null, new_line: 1 }, harness: "codex" as const };
  const { onAct } = mount({ ...noThreadsView, selected_path: "image.bin", ai: { ...view.ai, questions: [failed] } });
  fireEvent.click(screen.getByText("Review questions (1)"));
  fireEvent.click(screen.getByRole("button", { name: "Retry question" }));
  expect(screen.getByText("Question about code.rs, line 1 – line 1")).toBeTruthy();
  expect((screen.getByRole("combobox", { name: "Answering harness" }) as HTMLSelectElement).value).toBe("codex");
  fireEvent.click(screen.getByRole("button", { name: "Ask review question" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "ask", path: "code.rs", start: failed.start, end: failed.end, question: "Why this line?", harness: "codex" }));
});

it("drops a line selection when the selected file's patch changes, but keeps it across unrelated updates", () => {
  const { update } = mount(noThreadsView);
  fireEvent.click(screen.getAllByRole("button", { name: "Select line 1" })[0]);
  expect(screen.getByText("Selected line 1 – line 1")).toBeTruthy();
  update({ view: { ...noThreadsView, revision: 5 } });
  expect(screen.getByText("Selected line 1 – line 1")).toBeTruthy();
  const shifted = { ...noThreadsView.files[0], diff: { ...noThreadsView.files[0].diff, patch: "refreshed patch" } };
  update({ view: { ...noThreadsView, revision: 6, files: [shifted, noThreadsView.files[1]] } });
  expect(screen.queryByText("Selected line 1 – line 1")).toBeNull();
});

it("does not claim the review is updating while a pre-call notice waits on the reviewer", () => {
  const { update } = mount({ ...view, ai: { ...view.ai, precall: { title: "Walkthrough", harness: "Claude", preview: "", viewing: false } } });
  expect(screen.queryByText(/Updating review/)).toBeNull();
  update({ busy: true });
  expect(screen.getByText(/Updating review/)).toBeTruthy();
});

const answeredTurn = { question: "Why this line?", answer: "Repository explanation", error: null, focus: "code.rs, new line 1", path: "code.rs", start: { old_line: null, new_line: 1 }, end: { old_line: null, new_line: 1 }, harness: "codex" as const };
const withDraft: ReviewView = { ...noThreadsView, ai: { ...view.ai, questions: [answeredTurn], comment_draft: { request: 2, turn: 0, destination: "inline", text: "Generated feedback" } } };
const withReady: ReviewView = { ...noThreadsView, ai: { ...view.ai, questions: [answeredTurn], ready_comment: { request: 3, original: "Existing thread", path: "code.rs", start: answeredTurn.start, end: answeredTurn.end, text: "Existing thread\n\nGenerated feedback", severity: "nit" } } };

it("drafts from the selected answer with explicit inline or overall destinations", async () => {
  const { onAct, update } = mount({ ...noThreadsView, ai: { ...view.ai, questions: [answeredTurn] } });
  fireEvent.click(screen.getByText("Review questions (1)"));
  fireEvent.click(screen.getByRole("button", { name: "Draft inline comment" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "draft_question", turn: 0, destination: "inline" }));
  fireEvent.click(screen.getByRole("button", { name: "Draft overall feedback" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "draft_question", turn: 0, destination: "general" }));
  update({ view: { ...noThreadsView, ai: { ...view.ai, questions: [{ ...answeredTurn, start: null, end: null }] } } });
  expect((screen.getByRole("button", { name: "Draft inline comment" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "Draft overall feedback" }) as HTMLButtonElement).disabled).toBe(false);
});

it("retains edited AI draft text across polls and failed transfers and blocks duplicate submissions", async () => {
  const { onAct, update } = mount(withDraft, vi.fn(async () => false));
  const textbox = screen.getByRole("textbox", { name: "AI comment draft" });
  fireEvent.change(textbox, { target: { value: "Human edits 🦀\nMore feedback" } });
  update({ view: { ...withDraft, revision: 5, ai: { ...withDraft.ai, comment_draft: { ...withDraft.ai.comment_draft! } } } });
  expect((textbox as HTMLTextAreaElement).value).toBe("Human edits 🦀\nMore feedback");
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Open comment editor" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "transfer_question_draft", request: 2, text: "Human edits 🦀\nMore feedback" }));
  update({ busy: true });
  fireEvent.click(screen.getByRole("button", { name: "Open comment editor" }));
  expect(onAct).toHaveBeenCalledOnce();
  update({ error: "Repository changed", view: { ...withDraft, revision: 6 } });
  expect((textbox as HTMLTextAreaElement).value).toBe("Human edits 🦀\nMore feedback");
  expect(screen.getByRole("alert").textContent).toContain("Repository changed");
});

it("requires explicit discard before leaving a generated draft and preserves it when dismissal is cancelled", async () => {
  const { onAct } = mount(withDraft);
  fireEvent.click(screen.getByRole("button", { name: "Pause review" }));
  expect(screen.getByRole("alertdialog", { name: "Discard unsaved review draft" })).toBeTruthy();
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  expect((screen.getByRole("textbox", { name: "AI comment draft" }) as HTMLTextAreaElement).value).toBe("Generated feedback");
  fireEvent.click(screen.getByRole("button", { name: "Discard AI draft" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard and continue" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "discard_question_draft" }));
  expect(screen.queryByRole("textbox", { name: "AI comment draft" })).toBeNull();
});

it("opens the transferred inline editor without saving and keeps edits until an explicit anchored save", async () => {
  const { onAct, update } = mount(withReady, vi.fn(async () => false));
  const textbox = screen.getByRole("textbox", { name: "Line comment" });
  expect((textbox as HTMLTextAreaElement).value).toBe("Existing thread\n\nGenerated feedback");
  expect((screen.getByRole("combobox", { name: "Severity" }) as HTMLSelectElement).value).toBe("nit");
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.change(textbox, { target: { value: "Final feedback" } });
  update({ view: { ...withReady, revision: 8, ai: { ...withReady.ai, ready_comment: { ...withReady.ai.ready_comment! } } } });
  expect((textbox as HTMLTextAreaElement).value).toBe("Final feedback");
  fireEvent.click(screen.getByRole("button", { name: "Save comment" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "line_comment", path: "code.rs", start: answeredTurn.start, end: answeredTurn.end, text: "Final feedback", severity: "nit" }));
  expect((textbox as HTMLTextAreaElement).value).toBe("Final feedback");
});

it("opens transferred overall feedback and cancels without saving or restoring the draft on polling", async () => {
  const general = { ...withReady, ai: { ...withReady.ai, ready_comment: { ...withReady.ai.ready_comment!, path: null, start: null, end: null } } };
  const { onAct, update } = mount(general);
  expect(screen.getByRole("textbox", { name: "Overall feedback draft" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Cancel edit" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard and continue" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "discard_question_draft" }));
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "Overall feedback draft" })).toBeNull());
  update({ view: { ...general, revision: 9, ai: { ...general.ai, ready_comment: { ...general.ai.ready_comment! } } } });
  expect(screen.queryByRole("textbox", { name: "Overall feedback draft" })).toBeNull();
});

it("blocks drafting with disabled harnesses and while another request or unsaved draft is active", () => {
  const { update } = mount({ ...noThreadsView, ai: { ...view.ai, harnesses: ["claude"], questions: [answeredTurn] } });
  fireEvent.click(screen.getByText("Review questions (1)"));
  expect((screen.getByRole("button", { name: "Draft overall feedback" }) as HTMLButtonElement).disabled).toBe(true);
  update({ view: { ...noThreadsView, ai: { ...view.ai, running: true, question_running: true, questions: [answeredTurn] } } });
  expect((screen.getByRole("button", { name: "Draft inline comment" }) as HTMLButtonElement).disabled).toBe(true);
  update({ view: withDraft });
  expect((screen.getByRole("button", { name: "Draft overall feedback" }) as HTMLButtonElement).disabled).toBe(true);
});


it("protects a cleared transferred editor when an empty save would remove existing prose", async () => {
  const { onAct } = mount(withReady, vi.fn(async () => false));
  fireEvent.change(screen.getByRole("textbox", { name: "Line comment" }), { target: { value: "" } });
  fireEvent.click(screen.getByRole("button", { name: "Pause review" }));
  expect(screen.getByRole("alertdialog", { name: "Discard unsaved review draft" })).toBeTruthy();
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  fireEvent.click(screen.getByRole("button", { name: "Save comment" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "line_comment", path: "code.rs", start: answeredTurn.start, end: answeredTurn.end, text: "", severity: "nit" }));
});

it("previews the configured check and cancelling its confirmation launches nothing", () => {
  const { onAct } = mount({ ...view, summary, check_command: "npm test" });
  fireEvent.click(screen.getByRole("button", { name: "Run project check" }));
  const dialog = screen.getByRole("alertdialog", { name: "Run project review check" });
  expect(dialog.textContent).toContain("npm test");
  expect(dialog.textContent).toContain("may write build or test artifacts");
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel check launch" }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(onAct).not.toHaveBeenCalled();
});

it("submits a confirmed check once and retains a failed launch confirmation for retry", async () => {
  let resolve!: (value: boolean) => void;
  const onAct = vi.fn(() => new Promise<boolean>((done) => { resolve = done; }));
  const { update } = mount({ ...view, summary, check_command: "cargo test" }, onAct);
  fireEvent.click(screen.getByRole("button", { name: "Run project check" }));
  fireEvent.click(screen.getByRole("button", { name: "Run check now" }));
  fireEvent.click(screen.getByRole("button", { name: "Run check now" }));
  expect(onAct).toHaveBeenCalledTimes(1);
  expect(onAct).toHaveBeenCalledWith({ kind: "run_check", command: "cargo test" });
  resolve(false);
  await waitFor(() => expect((screen.getByRole("button", { name: "Run check now" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Run check now" }));
  resolve(true);
  update({ view: { ...view, summary, revision: 5, check_command: "cargo test", check: { command: "cargo test", status: "running", output: "" } } });
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(screen.getByText("Check running: cargo test")).toBeTruthy();
  expect((screen.getByRole("button", { name: "Apply pending suggestions" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Cancel running check" }));
  expect(onAct).toHaveBeenLastCalledWith({ kind: "cancel_check" });
});

it("displays pass fail and stale check output while leaving the summary open", () => {
  const { update } = mount({ ...view, summary, check_command: "test script", check: { command: "test script", status: "failed", output: "test diagnostic\nstderr" } });
  expect(screen.getByText("Check failed: test script")).toBeTruthy();
  expect(screen.getByLabelText("Project check output").textContent).toBe("test diagnostic\nstderr");
  expect((screen.getByRole("button", { name: "Run project check" }) as HTMLButtonElement).disabled).toBe(false);
  update({ view: { ...view, summary, check_command: "test script", check: { command: "test script", status: "passed", output: "all passed" } } });
  expect(screen.getByText("Check passed: test script")).toBeTruthy();
  update({ view: { ...view, summary, check_command: "test script", check: { command: "test script", status: "stale", output: "Changes changed; result discarded" } } });
  expect(screen.getByText("Check stale: test script")).toBeTruthy();
  expect(screen.getByRole("region", { name: "Pre-finish review summary" })).toBeTruthy();
});

it("requires a new confirmation when the configured command or revision changes", () => {
  const { onAct, update } = mount({ ...view, summary, check_command: "old command" });
  fireEvent.click(screen.getByRole("button", { name: "Run project check" }));
  update({ view: { ...view, summary, check_command: "new command" } });
  expect(screen.queryByRole("alertdialog")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Run project check" }));
  expect(screen.getByRole("alertdialog").textContent).toContain("new command");
  update({ view: { ...view, summary, revision: 5, check_command: "new command" } });
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(onAct).not.toHaveBeenCalled();
});

it("retains unsaved drafts through check polling and blocks launch until they are saved or discarded", async () => {
  const { onAct, update } = mount({ ...view, check_command: "true" });
  fireEvent.click(screen.getByRole("button", { name: "Edit file comment" }));
  fireEvent.change(screen.getByRole("textbox", { name: "File comment" }), { target: { value: "Unsent check draft" } });
  update({ view: { ...view, summary, check_command: "true" } });
  expect((screen.getByRole("button", { name: "Run project check" }) as HTMLButtonElement).disabled).toBe(true);
  update({ view: { ...view, summary, revision: 5, check_command: "true", check: { command: "true", status: "passed", output: "" } } });
  fireEvent.click(screen.getByRole("button", { name: "Return to review" }));
  await waitFor(() => expect(onAct).toHaveBeenLastCalledWith({ kind: "summary_close" }));
  update({ view: { ...view, check_command: "true" } });
  expect((screen.getByRole("textbox", { name: "File comment" }) as HTMLTextAreaElement).value).toBe("Unsent check draft");
});

it("blocks check launches during AI or save failures and reports missing configuration", () => {
  const { update } = mount({ ...view, summary });
  expect(screen.getByText("No final review check is configured for this project.")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Run project check" })).toBeNull();
  update({ view: { ...view, summary, check_command: "true", ai: { ...view.ai, running: true } } });
  expect((screen.getByRole("button", { name: "Run project check" }) as HTMLButtonElement).disabled).toBe(true);
  update({ view: { ...view, summary, check_command: "true", save_error: "disk full" } });
  expect((screen.getByRole("button", { name: "Run project check" }) as HTMLButtonElement).disabled).toBe(true);
});

const finish: NonNullable<ReviewView["finish"]> = {
  approved: 1, needs_work: 1, skipped: 1, file_comments: 1, line_comments: 1, general_feedback: true, apply_suggestions: 2,
  post_to_pr: true, submit_prompt: true, handoff: { session_id: "agent-1", label: "Claude 1", stopped: false }, completing: false,
};

it("shows what completing records and hands off, and cancelling or closing sends nothing", () => {
  const { onAct } = mount({ ...view, summary, finish, check_command: "cargo test" });
  expect(screen.getByText(/1 approved · 1 need work · 1 skipped/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Complete review…" }));
  const confirm = screen.getByRole("alertdialog", { name: "Complete Final Review" });
  for (const text of [/1 file\(s\) have no verdict and are recorded as skipped/, /2 saved replacement\(s\) are written to source first/,
    /Runs the configured check first/, /earlier results shown here are not reused/, /GitHub pull request/,
    /sends the "address the feedback" prompt to Claude 1/]) {
    expect(within(confirm).getByText(text)).toBeTruthy();
  }
  expect(within(confirm).getByText("cargo test")).toBeTruthy();
  fireEvent.click(within(confirm).getByRole("button", { name: "Keep reviewing" }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(onAct).not.toHaveBeenCalled();
});

it("sends one completion with the confirmed expectations and offers completing without handoff", async () => {
  let resolve!: (value: boolean) => void;
  const onAct = vi.fn(() => new Promise<boolean>((done) => { resolve = done; }));
  mount({ ...view, summary, finish, check_command: "cargo test" }, onAct);
  fireEvent.click(screen.getByRole("button", { name: "Complete review…" }));
  fireEvent.click(screen.getByRole("button", { name: "Complete without handoff" }));
  fireEvent.click(screen.getByRole("button", { name: "Complete and hand off to Claude 1" }));
  expect(onAct).toHaveBeenCalledTimes(1);
  expect(onAct).toHaveBeenCalledWith({ kind: "complete", check_command: "cargo test", apply_suggestions: 2, handoff_session: "agent-1", deliver: false });
  // A refused completion keeps the confirmation for another decision.
  resolve(false);
  await waitFor(() => expect((screen.getByRole("button", { name: "Complete and hand off to Claude 1" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Complete and hand off to Claude 1" }));
  expect(onAct).toHaveBeenLastCalledWith({ kind: "complete", check_command: "cargo test", apply_suggestions: 2, handoff_session: "agent-1", deliver: true });
});

it("explains stopped and missing handoff targets", () => {
  const stopped = { ...finish, handoff: { session_id: "agent-1", label: "Claude 1", stopped: true } };
  const { update } = mount({ ...view, summary, finish: stopped });
  fireEvent.click(screen.getByRole("button", { name: "Complete review…" }));
  expect(screen.getByText(/unsent draft in Claude 1 \(stopped, so it cannot be sent yet\)/)).toBeTruthy();
  expect(screen.getByText("No project check is configured.")).toBeTruthy();
  update({ view: { ...view, summary, finish: { ...finish, handoff: null }, revision: 9 } });
  fireEvent.click(screen.getByRole("button", { name: "Complete review…" }));
  expect(screen.getByText(/no agent session; the feedback is saved for later/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: /hand off/ })).toBeNull();
  expect(screen.getByRole("button", { name: "Complete review" })).toBeTruthy();
});

it("keeps unsaved and generated drafts by refusing to complete until they are settled", () => {
  const { update } = mount();
  fireEvent.click(screen.getByRole("button", { name: "Overall feedback" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Overall feedback draft" }), { target: { value: "Unsaved overall" } });
  update({ view: { ...view, summary, finish } });
  expect((screen.getByRole("button", { name: "Complete review…" }) as HTMLButtonElement).disabled).toBe(true);
  update({ view });
  expect((screen.getByRole("textbox", { name: "Overall feedback draft" }) as HTMLTextAreaElement).value).toBe("Unsaved overall");
  cleanup();
  mount({ ...view, summary, finish, ai: { ...view.ai, comment_draft: { request: 1, turn: 0, destination: "general", text: "Generated" } } });
  expect((screen.getByRole("button", { name: "Complete review…" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText("Transfer or discard the generated comment draft before completing.")).toBeTruthy();
});

it("cancels a completion while its check runs and cannot leave the summary meanwhile", async () => {
  const { onAct } = mount({ ...view, summary, finish: { ...finish, completing: true }, check_command: "cargo test",
    check: { command: "cargo test", status: "running", output: "" } });
  expect(screen.queryByRole("button", { name: "Complete review…" })).toBeNull();
  expect((screen.getByRole("button", { name: "Return to review" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText(/Completion waits for this check/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Cancel completion" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "cancel_check" }));
});
