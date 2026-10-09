// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import PrReviewPanel from "../src/PrReviewPanel";
import type { PrReviewAction, PrReviewView } from "../src/prReviewApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
const base: PrReviewView = {
  workflow_id: "review", revision: 1, project_name: "Demo", stage: "pick", loading: false,
  entries: [{ number: 9, title: "Teammate change", author: "alice", head_ref: "topic", is_draft: false, has_draft: true, updated: true }],
  number: null, title: null, files: [], summary: "", submission: null, error: null, notice: null,
};
const review: PrReviewView = { ...base, stage: "review", number: 9, title: "Teammate change", files: [{
  diff: { path: "lib.rs", old_path: null, status: "modified", additions: 1, deletions: 1, is_binary: false, hunks: [], patch: "-old\n+new" }, comment: "",
}] };
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
