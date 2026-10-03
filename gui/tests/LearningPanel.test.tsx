// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import LearningPanel from "../src/LearningPanel";
import type { LearningView } from "../src/api";

afterEach(cleanup);
const view: LearningView = {
  workflow_id: "reader-1", revision: 0, target: { project_id: "p", feature_id: "f" },
  feature_name: "Feature", scope: "repo_tree", is_git: true,
  entries: [{ key: "file:README.md", label: "README.md", kind: "file", depth: 0, expanded: false }],
  content_path: "README.md", content: ["first", "second", "third"], content_line_labels: ["1", "2", "3"],
  content_error: null, anchor: "all of README.md", selection: null, hunks: [], starters: [], can_keep_todo: true,
  harness: "claude", harnesses: ["claude", "codex", "opencode", "pi"],
  level: "newcomer", history_saved: true, error: null, notice: null,
  qa: [{ id: "qa-1", parent_id: null, question: "Original question", answer: "**Original answer**", anchor: "line 2 of README.md",
    status: "answered", intent: "explain", run_mode: "this file only", harness: "claude", error: null, drift: null, spawned_session_id: null,
    todo_id: null, todo_seed: { title: "Original answer", notes: "Seeded notes" } }],
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

it("fills the question from a starter without asking it", () => {
  const onAct = vi.fn(async () => true);
  const starters = [{ text: "What would break if I deleted this?", intent: "explain" as const },
    { text: "Suggest how to make this clearer without changing behaviour.", intent: "action" as const }];
  render(<LearningPanel view={{ ...view, starters }} busy={false} onAct={onAct} onClose={vi.fn()} onLaunch={vi.fn()} />);
  const group = screen.getByRole("group", { name: "Starter questions" });
  fireEvent.click(within(group).getByRole("button", { name: starters[1].text }));
  expect((screen.getByRole("textbox", { name: "Question" }) as HTMLTextAreaElement).value).toBe(starters[1].text);
  expect((screen.getByRole("combobox", { name: "Question intent" }) as HTMLSelectElement).value).toBe("action");
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Follow up" }));
  expect(screen.queryByRole("group", { name: "Starter questions" })).toBeNull();
});

it("selects a whole hunk and highlights the rows it covers", () => {
  const onAct = vi.fn(async () => true);
  const diff: LearningView = { ...view, scope: "branch_changes", content: [" first", "-second", "+changed", " third"],
    content_line_labels: ["1→1", "2→−", "+2", "3→3"], hunks: [{ index: 0, start: 1, end: 4 }], selection: [2, 3] };
  render(<LearningPanel view={diff} busy={false} onAct={onAct} onClose={vi.fn()} onLaunch={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Select hunk 1" }));
  expect(onAct).toHaveBeenLastCalledWith({ kind: "hunk_anchor", index: 0 });
  const selected = [1, 2, 3, 4].map((line) =>
    screen.getByRole("button", { name: `Select line ${line}` }).classList.contains("learning-line-selected"));
  expect(selected).toEqual([false, true, true, false]);
});

it("re-files an answer's intent and keeps it as an edited TODO", async () => {
  const onAct = vi.fn(async () => true);
  const { rerender } = render(<LearningPanel view={view} busy={false} onAct={onAct} onClose={vi.fn()} onLaunch={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Re-file as change request" }));
  expect(onAct).toHaveBeenLastCalledWith({ kind: "relabel_intent", qa_id: "qa-1" });

  fireEvent.click(screen.getByRole("button", { name: "Keep as TODO" }));
  const form = screen.getByRole("form", { name: "Keep as TODO" });
  const title = within(form).getByRole("textbox", { name: "TODO title" }) as HTMLInputElement;
  expect(title.value).toBe("Original answer");
  fireEvent.change(title, { target: { value: "Read the parser" } });
  fireEvent.change(within(form).getByRole("textbox", { name: "TODO notes" }), { target: { value: "My notes" } });
  fireEvent.click(within(form).getByRole("button", { name: "Save TODO" }));
  expect(onAct).toHaveBeenLastCalledWith({ kind: "keep_todo", qa_id: "qa-1", title: "Read the parser", notes: "My notes" });
  await waitFor(() => expect(screen.queryByRole("form", { name: "Keep as TODO" })).toBeNull());

  const kept = { ...view, qa: [{ ...view.qa[0], todo_id: "todo-1" }] };
  rerender(<LearningPanel view={kept} busy={false} onAct={onAct} onClose={vi.fn()} onLaunch={vi.fn()} />);
  expect(screen.getByText("On the TODO list")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Keep as TODO" })).toBeNull();
});

it("keeps the TODO form open when saving is refused", async () => {
  render(<LearningPanel view={view} busy={false} onAct={vi.fn(async () => false)} onClose={vi.fn()} onLaunch={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Keep as TODO" }));
  fireEvent.click(screen.getByRole("button", { name: "Save TODO" }));
  await waitFor(() => expect(screen.getByRole("form", { name: "Keep as TODO" })).toBeTruthy());
});

it("hands a failed question to an agent and offers to return to a linked one", () => {
  const onLaunch = vi.fn();
  const failed = { ...view, qa: [{ ...view.qa[0], status: "failed" as const, answer: null, todo_seed: null }] };
  const { rerender } = render(<LearningPanel view={failed} busy={false} onAct={vi.fn(async () => true)} onClose={vi.fn()} onLaunch={onLaunch} />);
  fireEvent.click(screen.getByRole("button", { name: "Open editing agent" }));
  expect(onLaunch).toHaveBeenCalledWith("qa-1");
  expect((screen.getByRole("button", { name: "Keep as TODO" }) as HTMLButtonElement).disabled).toBe(true);
  const linked = { ...view, qa: [{ ...view.qa[0], spawned_session_id: "agent" }] };
  rerender(<LearningPanel view={linked} busy={false} onAct={vi.fn(async () => true)} onClose={vi.fn()} onLaunch={onLaunch} />);
  expect(screen.getByRole("button", { name: "Return to editing agent" })).toBeTruthy();
});
