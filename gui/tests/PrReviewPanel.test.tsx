// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import PrReviewPanel from "../src/PrReviewPanel";
import type { PrReviewAction, PrReviewView } from "../src/prReviewApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
const base: PrReviewView = {
  workflow_id: "review", revision: 1, project_name: "Demo", head_sha: "abcdef0123456789", stage: "pick", loading: false,
  entries: [{ number: 9, title: "Teammate change", author: "alice", head_ref: "topic", is_draft: false, has_draft: true, updated: true }],
  number: null, title: null, files: [], summary: "", submission: null, error: null, notice: null,
};
const review: PrReviewView = { ...base, stage: "review", number: 9, title: "Teammate change", files: [{
  diff: { path: "lib.rs", old_path: null, status: "modified", additions: 1, deletions: 1, is_binary: false, hunks: [], patch: "-old\n+new" }, comment: "", line_comments: [],
}] };
const line = (kind: "context" | "added" | "removed", old_line: number | null, new_line: number | null, text: string) => ({ kind, old_line, new_line, text });
const HUNKS = [
  { header: "@@ -5,3 +5,3 @@", lines: [line("context", 5, 5, " five"), line("removed", 6, null, "-six"), line("added", null, 6, "+SIX"), line("context", 7, 7, " seven")] },
  { header: "@@ -30,2 +30,2 @@", lines: [line("context", 30, 30, " thirty"), line("added", null, 31, "+late")] },
];
const withHunks = (threads: PrReviewView["files"][0]["line_comments"] = []): PrReviewView => ({ ...review,
  files: [{ ...review.files[0], diff: { ...review.files[0].diff, hunks: HUNKS }, line_comments: threads }] });
const loc = (old_line: number | null, new_line: number | null) => ({ old_line, new_line });
const pick = (label: string, shift = false) => fireEvent.click(screen.getByRole("button", { name: label }), { shiftKey: shift });
const thread = (over: Partial<PrReviewView["files"][0]["line_comments"][0]> = {}) => ({
  start: loc(5, 5), end: loc(5, 5), side: "RIGHT" as const, start_side: null, editable: true, anchor: "line 5", text: "Saved note",
  severity: "nit" as const, resolved: false, anchor_lost: false, suggestion: null, inline: true, note: null, ...over });
const actions = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "pr_review_act").map(([, args]) => (args as { action: PrReviewAction }).action);

it("opens a selected PR, saves a draft and previews the exact submission before posting", async () => {
  let current = base;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "pr_review_begin") return base;
    const action = (args as { action: PrReviewAction }).action;
    if (action.kind === "open") current = review;
    if (action.kind === "summary") current = { ...current, summary: action.text };
    if (action.kind === "preview") current = { ...current, submission: { event: action.event, body: current.summary, comments: [], file_comments: [], posting: false, error: null, head_moved: false } };
    if (action.kind === "confirm_submit") current = { ...base, notice: "Posted review" };
    return { ...current, revision: current.revision + 1 };
  });
  render(<PrReviewPanel projectId="project" onClose={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: /#9 Teammate change/ }));
  const summary = await screen.findByRole("textbox", { name: "Review summary" });
  fireEvent.change(summary, { target: { value: "Looks good" } });
  expect((screen.getByRole("button", { name: "Preview submission" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Save summary" }));
  await waitFor(() => expect((screen.getByRole("button", { name: "Preview submission" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "Preview submission" }));
  expect(await screen.findByRole("region", { name: "Confirm PR review" })).toBeTruthy();
  expect(actions().some((a) => a.kind === "confirm_submit")).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "Confirm and post to GitHub" }));
  expect(await screen.findByText("Posted review")).toBeTruthy();
  expect(actions()).toEqual([{ kind: "open", number: 9 }, { kind: "summary", text: "Looks good" }, { kind: "preview", event: "COMMENT" }, { kind: "confirm_submit" }]);
});

it("keeps unsaved editor text on a failed save and confirms discarding it before leaving", async () => {
  const close = vi.fn();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "pr_review_begin" || command === "pr_review_snapshot") return review;
    if ((args as { action: PrReviewAction }).action.kind === "close") return null;
    throw { kind: "internal", message: "Draft storage unavailable" };
  });
  render(<PrReviewPanel projectId="project" onClose={close} />);
  const comment = await screen.findByRole("textbox", { name: "Comment on lib.rs" });
  fireEvent.change(comment, { target: { value: "Keep this" } });
  fireEvent.click(screen.getByRole("button", { name: "Save file comment" }));
  await screen.findByText("Draft storage unavailable");
  expect((comment as HTMLTextAreaElement).value).toBe("Keep this");
  fireEvent.click(screen.getByRole("button", { name: "Close" }));
  expect(close).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Discard and leave" }));
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
});

it.each([
  { kind: "summary", label: "Review summary", save: "Save summary" },
  { kind: "file_comment", label: "Comment on lib.rs", save: "Save file comment" },
] as const)("allows retrying a failed $kind save when the snapshot contains unpersisted text", async ({ kind, label, save }) => {
  let current = review;
  let failed = false;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "pr_review_begin") return review;
    if (command === "pr_review_snapshot") return structuredClone(current);
    const action = (args as { action: PrReviewAction }).action;
    if (action.kind === kind && "text" in action) {
      current = { ...current, revision: current.revision + 1,
        summary: kind === "summary" ? action.text : current.summary,
        files: current.files.map((f) => ({ ...f, comment: kind === "file_comment" ? action.text : f.comment })),
      };
      if (!failed) {
        failed = true;
        throw { kind: "internal", message: "Draft storage unavailable" };
      }
    }
    return current;
  });
  render(<PrReviewPanel projectId="project" onClose={vi.fn()} />);
  const editor = await screen.findByRole("textbox", { name: label });
  fireEvent.change(editor, { target: { value: "Retry this draft" } });
  fireEvent.click(screen.getByRole("button", { name: save }));
  await screen.findByText("Draft storage unavailable");
  await waitFor(() => expect((screen.getByRole("button", { name: save }) as HTMLButtonElement).disabled).toBe(false));
  expect((editor as HTMLTextAreaElement).value).toBe("Retry this draft");
  expect((screen.getByRole("button", { name: "Preview submission" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Back to pull requests" }));
  expect(screen.getByText(/Discard unsaved editor text/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  fireEvent.click(screen.getByRole("button", { name: save }));
  await waitFor(() => expect((screen.getByRole("button", { name: "Preview submission" }) as HTMLButtonElement).disabled).toBe(false));
  expect((screen.getByRole("button", { name: save }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByText("Draft storage unavailable")).toBeNull();
  const saves = vi.mocked(invoke).mock.calls.filter(([command]) => command === "pr_review_act")
    .map(([, args]) => args as { revision: number; action: PrReviewAction });
  expect(saves.map(({ revision }) => revision)).toEqual([1, 2]);
});

it("shows the side and full line range of each inline comment before posting", async () => {
  vi.mocked(invoke).mockResolvedValue({ ...review, submission: {
    event: "COMMENT", body: "Review summary", posting: false, error: null, head_moved: false,
    comments: [
      { path: "lib.rs", line: 8, start_line: null, side: "LEFT", body: "Base line" },
      { path: "lib.rs", line: 12, start_line: 10, side: "LEFT", body: "Base range" },
      { path: "lib.rs", line: 20, start_line: null, side: "RIGHT", body: "Current line" },
      { path: "lib.rs", line: 25, start_line: 22, side: "RIGHT", body: "```suggestion\nreplacement\n```" },
    ],
    file_comments: [{ path: "other.rs", body: "File comment" }],
  } });
  render(<PrReviewPanel projectId="project" onClose={vi.fn()} />);
  await screen.findByRole("region", { name: "Confirm PR review" });
  for (const target of ["lib.rs:8 (base · LEFT)", "lib.rs:10–12 (base · LEFT)", "lib.rs:20 (current · RIGHT)", "lib.rs:22–25 (current · RIGHT)", "other.rs"]) {
    expect(screen.getByText(target)).toBeTruthy();
  }
  expect(screen.getByText("replacement")).toBeTruthy();
  expect(actions()).toEqual([]);
});

it("selects a line or a Shift-click range and saves a line comment with its severity and span", async () => {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "pr_review_begin") return withHunks();
    const action = (args as { action: PrReviewAction }).action;
    return withHunks(action.kind === "line_comment" ? [thread({ start: action.start, end: action.end, text: action.text })] : []);
  });
  render(<PrReviewPanel projectId="project" onClose={vi.fn()} />);
  await screen.findByText(/pinned to head/);
  pick("Select line 5");
  pick("Select line 6", true);
  expect(screen.getByText("Selected line 5 – line 6")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Comment on selection" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Line comment" }), { target: { value: "Why this?" } });
  fireEvent.change(screen.getByRole("combobox", { name: "Severity" }), { target: { value: "question" } });
  fireEvent.click(screen.getByRole("button", { name: "Save line comment" }));
  await screen.findByText(/Saved note|Why this/);
  expect(actions()).toEqual([{ kind: "line_comment", path: "lib.rs", start: loc(5, 5), end: loc(null, 6), text: "Why this?", severity: "question" }]);
  expect(screen.queryByRole("textbox", { name: "Line comment" })).toBeNull();
});

it("anchors a comment on a removed line to the base side and refuses suggestions that include it", async () => {
  vi.mocked(invoke).mockResolvedValue(withHunks());
  render(<PrReviewPanel projectId="project" onClose={vi.fn()} />);
  await screen.findByText(/pinned to head/);
  pick("Select base line 6");
  expect(screen.getByText("Selected base line 6 – base line 6")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Suggest replacement" }));
  expect((await screen.findByRole("alert")).textContent).toMatch(/removed \(base-only\) line/);
  expect(screen.queryByRole("textbox", { name: "Suggested replacement" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Comment on selection" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Line comment" }), { target: { value: "Gone" } });
  fireEvent.click(screen.getByRole("button", { name: "Save line comment" }));
  await waitFor(() => expect(actions()).toEqual([{ kind: "line_comment", path: "lib.rs", start: loc(6, null), end: loc(6, null), text: "Gone", severity: "suggestion" }]));
});

it("refuses a range that crosses hunks before sending anything", async () => {
  vi.mocked(invoke).mockResolvedValue(withHunks());
  render(<PrReviewPanel projectId="project" onClose={vi.fn()} />);
  await screen.findByText(/pinned to head/);
  pick("Select line 7");
  pick("Select line 31", true);
  fireEvent.click(screen.getByRole("button", { name: "Comment on selection" }));
  expect((await screen.findByRole("alert")).textContent).toMatch(/single hunk/);
  expect(actions()).toEqual([]);
});

it("seeds a suggestion with the current lines and sends the exact span", async () => {
  vi.mocked(invoke).mockResolvedValue(withHunks());
  render(<PrReviewPanel projectId="project" onClose={vi.fn()} />);
  await screen.findByText(/pinned to head/);
  pick("Select line 6");
  pick("Select line 7", true);
  fireEvent.click(screen.getByRole("button", { name: "Suggest replacement" }));
  const box = await screen.findByRole("textbox", { name: "Suggested replacement" }) as HTMLTextAreaElement;
  expect(box.value).toBe("SIX\nseven");
  fireEvent.change(box, { target: { value: "six\nseven" } });
  fireEvent.click(screen.getByRole("button", { name: "Save suggestion" }));
  await waitFor(() => expect(actions()).toEqual([{ kind: "suggestion", path: "lib.rs", start: loc(null, 6), end: loc(7, 7), text: "six\nseven" }]));
});

it("shows saved threads, where they post, and resolves one without a GitHub call", async () => {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "pr_review_begin") return withHunks([thread({ suggestion: "fixed", note: null }), thread({ start: loc(30, 30), end: loc(30, 30), anchor: "line 30", text: "Stray", inline: false, note: "It is outside the PR's diff; it will be added to the review summary" })]);
    const action = (args as { action: PrReviewAction }).action;
    return withHunks([thread({ resolved: action.kind === "toggle_line_resolved", note: "Resolved threads are kept locally and not posted" })]);
  });
  render(<PrReviewPanel projectId="project" onClose={vi.fn()} />);
  expect(await screen.findByText(/outside the PR's diff/)).toBeTruthy();
  expect(screen.getByText("fixed")).toBeTruthy();
  fireEvent.click(screen.getAllByRole("button", { name: "Resolve thread" })[0]);
  expect(await screen.findByText(/Resolved threads are kept locally/)).toBeTruthy();
  expect(actions()).toEqual([{ kind: "toggle_line_resolved", path: "lib.rs", start: loc(5, 5), end: loc(5, 5) }]);
  expect(vi.mocked(invoke).mock.calls.some(([command, args]) => command === "pr_review_act" && ["confirm_submit", "preview"].includes((args as { action: PrReviewAction }).action.kind))).toBe(false);
});

it("keeps the editor and shows the failure when saving a thread fails", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "pr_review_begin" || command === "pr_review_snapshot") return withHunks();
    throw { kind: "internal", message: "drafts are read-only" };
  });
  render(<PrReviewPanel projectId="project" onClose={vi.fn()} />);
  await screen.findByText(/pinned to head/);
  pick("Select line 5");
  fireEvent.click(screen.getByRole("button", { name: "Comment on selection" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Line comment" }), { target: { value: "Keep me" } });
  fireEvent.click(screen.getByRole("button", { name: "Save line comment" }));
  await screen.findByText("drafts are read-only");
  expect((screen.getByRole("textbox", { name: "Line comment" }) as HTMLTextAreaElement).value).toBe("Keep me");
});
