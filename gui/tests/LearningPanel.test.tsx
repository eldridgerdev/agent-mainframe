// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import LearningPanel from "../src/LearningPanel";
import type { LearningView } from "../src/api";

afterEach(cleanup);
const view: LearningView = {
  workflow_id: "reader-1", revision: 0, target: { project_id: "p", feature_id: "f" },
  feature_name: "Feature", scope: "repo_tree", is_git: true,
  entries: [{ key: "file:README.md", label: "README.md", kind: "file", depth: 0, expanded: false }],
  content_path: "README.md", content: ["first", "second", "third"], content_line_labels: ["1", "2", "3"],
  content_error: null, anchor: "all of README.md", harness: "claude", harnesses: ["claude", "codex", "opencode", "pi"],
  level: "newcomer", history_saved: true, error: null, notice: null,
  qa: [{ id: "qa-1", parent_id: null, question: "Original question", answer: "**Original answer**", anchor: "line 2 of README.md",
    status: "answered", intent: "explain", run_mode: "this file only", harness: "claude", error: null, drift: null, spawned_session_id: null }],
};

it("preserves unsent questions across polls and protects them on close", () => {
  const onAct = vi.fn(async () => true);
  const onClose = vi.fn();
  const props = { view, busy: false, onAct, onClose, onLaunch: vi.fn() };
  const { rerender } = render(<LearningPanel {...props} />);
  const input = screen.getByRole("textbox", { name: "Question" }) as HTMLTextAreaElement;
  fireEvent.change(input, { target: { value: "Unsaved question" } });
  rerender(<LearningPanel {...props} view={{ ...view, qa: [...view.qa] }} />);
  expect(input.value).toBe("Unsaved question");
  fireEvent.click(screen.getByRole("button", { name: "Close" }));
  expect(onClose).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Keep writing" }));
  expect(input.value).toBe("Unsaved question");
  fireEvent.click(screen.getByRole("button", { name: "Close" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard question and close" }));
  expect(onClose).toHaveBeenCalledOnce();
});

it("maps Shift-click to an inclusive range and submits a follow-up by stable answer id", async () => {
  const onAct = vi.fn(async () => true);
  render(<LearningPanel view={view} busy={false} onAct={onAct} onClose={vi.fn()} onLaunch={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Select line 1" }));
  fireEvent.click(screen.getByRole("button", { name: "Select line 3" }), { shiftKey: true });
  expect(onAct).toHaveBeenLastCalledWith({ kind: "lines_anchor", start: 1, end: 3 });
  fireEvent.click(screen.getByRole("button", { name: "Follow up" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Question" }), { target: { value: "Why?" } });
  fireEvent.click(screen.getByRole("button", { name: "Ask claude" }));
  expect(onAct).toHaveBeenLastCalledWith({ kind: "ask", question: "Why?", intent: "explain", parent_id: "qa-1" });
});

it("requires an explicit editing handoff and disables actions while one is pending", () => {
  const onLaunch = vi.fn();
  const props = { view, onAct: vi.fn(async () => true), onLaunch, onClose: vi.fn() };
  const { rerender } = render(<LearningPanel {...props} busy={false} />);
  expect(screen.getByText("Original answer").tagName).toBe("STRONG");
  expect(onLaunch).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Open editing agent" }));
  expect(onLaunch).toHaveBeenCalledWith("qa-1");
  rerender(<LearningPanel {...props} busy={true} />);
  expect((screen.getByRole("button", { name: "Open editing agent" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Open editing agent" }));
  expect(onLaunch).toHaveBeenCalledOnce();
});

it("keeps the question after a refused submission and states Codex's effective scope", async () => {
  render(<LearningPanel view={{ ...view, harness: "codex" }} busy={false} onAct={vi.fn(async () => false)} onClose={vi.fn()} onLaunch={vi.fn()} />);
  fireEvent.change(screen.getByRole("textbox", { name: "Question" }), { target: { value: "Keep this" } });
  fireEvent.click(screen.getByRole("button", { name: "Ask codex" }));
  expect((screen.getByRole("textbox", { name: "Question" }) as HTMLTextAreaElement).value).toBe("Keep this");
  expect(screen.getByText("Codex reads the repository in a read-only sandbox.")).toBeTruthy();
});
