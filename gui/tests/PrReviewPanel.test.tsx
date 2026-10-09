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
