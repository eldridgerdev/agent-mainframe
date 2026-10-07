// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import DiffPanel, { Hunk } from "../src/DiffPanel";
import LearningPanel from "../src/LearningPanel";
import ReviewPanel from "../src/ReviewPanel";
import { SyntaxBadge } from "../src/SyntaxCode";
import type { DiffFile, DiffHunk, DiffView, LearningView, ReviewView } from "../src/api";
import type { SyntaxInfo, SyntaxInstallView } from "../src/syntaxApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); vi.clearAllMocks(); vi.useRealTimers(); });

const rust: SyntaxInfo = { language: "Rust", language_key: "rust", status: "highlighted" };
// A removed line inside a block comment that opened outside the hunk is
// still a comment: the backend coloured it with whole-file context.
const hunk: DiffHunk = { header: "@@ -4,2 +4,2 @@", lines: [
  { kind: "context", text: " \tlet total = 1;", old_line: 4, new_line: 4,
    syntax: [["", " \t"], ["keyword", "let"], ["", " total = "], ["number", "1"], ["punctuation-delimiter", ";"]] },
  { kind: "removed", text: "-   still a comment */", old_line: 5, new_line: null, syntax: [["", "-"], ["comment", "   still a comment */"]] },
  { kind: "added", text: "+let s = \"multi", old_line: null, new_line: 5, syntax: [["", "+"], ["keyword", "let"], ["", " s = "], ["string", "\"multi"]] },
  { kind: "added", text: "+plain line", old_line: null, new_line: 6, syntax: null },
] };

function codeCell(text: string) {
  return Array.from(document.querySelectorAll("code")).find((code) => code.textContent === text)!;
}

it("colours unified rows from backend spans while keeping text, whitespace and markers exact", () => {
  render(<Hunk hunk={hunk} split={false} />);
  const context = codeCell(" \tlet total = 1;");
  expect(context.querySelector(".syn-keyword")?.textContent).toBe("let");
  expect(context.querySelector(".syn-number")?.textContent).toBe("1");
  const removed = codeCell("-   still a comment */");
  expect(removed.querySelector(".syn-comment")?.textContent).toBe("   still a comment */");
  expect(removed.closest("tr")?.className).toContain("diff-removed");
  expect(codeCell("+let s = \"multi").querySelector(".syn-string")?.textContent).toBe("\"multi");
  // A line without spans renders plain.
  expect(codeCell("+plain line").children).toHaveLength(0);
  // Both line-number columns stay beside the coloured code.
  expect(screen.getAllByText("4", { selector: "td" })).toHaveLength(2);
});

it("keeps line selection working on highlighted rows in both layouts", () => {
  const select = vi.fn();
  const selection = { disabled: false, contains: (line: { new_line: number | null }) => line.new_line === 5, select };
  const { rerender } = render(<Hunk hunk={hunk} split={false} selection={selection} />);
  fireEvent.click(screen.getByRole("button", { name: "Select line 5" }));
  expect(select).toHaveBeenCalledWith(hunk.lines[2], false);
  expect(codeCell("+let s = \"multi").closest("tr")?.className).toContain("review-line-selected");
  rerender(<Hunk hunk={hunk} split={true} selection={selection} />);
  const table = screen.getByRole("table", { name: "Side-by-side hunk" });
  const cell = codeCell("+let s = \"multi").closest("td")!;
  expect(cell.className).toContain("review-line-selected");
  expect(within(table).getByText("still a comment */", { exact: false }).closest("td")?.className).toContain("diff-removed");
  fireEvent.click(screen.getByRole("button", { name: "Select base line 5" }), { shiftKey: true });
  expect(select).toHaveBeenLastCalledWith(hunk.lines[1], true);
});

it("labels highlighted, plain and unsupported files and hides the badge for binaries", () => {
  const { rerender, container } = render(<SyntaxBadge info={rust} />);
  expect(screen.getByText("Rust").className).toContain("syntax-chip-on");
  rerender(<SyntaxBadge info={{ language: null, language_key: null, status: "unsupported" }} />);
  expect(screen.getByText("Plain text · no supported language")).toBeTruthy();
  rerender(<SyntaxBadge info={{ ...rust, status: "too_large" }} />);
  expect(screen.getByText("Plain text · file too large to highlight")).toBeTruthy();
  rerender(<SyntaxBadge info={{ ...rust, status: "binary" }} />);
  expect(container.textContent).toBe("");
  expect(invoke).not.toHaveBeenCalled();
});

const idle: SyntaxInstallView = { language: null, language_key: null, running: false, output: null, message: null, error: null, completed: 0 };

it("installs a missing parser only after explicit confirmation, then reloads the view once", async () => {
  let status: SyntaxInstallView = idle;
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "syntax_install_status") return status;
    if (command === "syntax_install") {
      status = { ...idle, language: "Python", language_key: "python", running: true, output: "$ git clone tree-sitter-python" };
      return status;
    }
    throw new Error(`unexpected ${command}`);
  });
  const onInstalled = vi.fn();
  render(<SyntaxBadge info={{ language: "Python", language_key: "python", status: "not_installed" }} onInstalled={onInstalled} />);
  expect(screen.getByText("Plain text · Python parser not installed")).toBeTruthy();
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("syntax_install_status"));
  fireEvent.click(screen.getByRole("button", { name: "Install Python parser…" }));
  const confirm = screen.getByRole("group", { name: "Install Python parser" });
  expect(within(confirm).getByText(/clones its tree-sitter grammar from GitHub/)).toBeTruthy();
  fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
  expect(invoke).not.toHaveBeenCalledWith("syntax_install", expect.anything());
  fireEvent.click(screen.getByRole("button", { name: "Install Python parser…" }));
  fireEvent.click(screen.getByRole("button", { name: "Install parser" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("syntax_install", { language: "python" }));
  expect(await screen.findByText("$ git clone tree-sitter-python")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Install Python parser…" })).toBeNull();
  status = { ...status, running: false, output: null, message: "Installed Python tree-sitter parser", completed: 1 };
  expect(await screen.findByText("Installed Python tree-sitter parser", {}, { timeout: 3000 })).toBeTruthy();
  expect(onInstalled).toHaveBeenCalledOnce();
});

it("reports a refused or failed install and does not reload", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "syntax_install_status") {
      return { ...idle, language: "Rust", language_key: "rust", error: "cc: not found", completed: 3 };
    }
    throw { kind: "conflict", message: "The Go parser is already being installed; wait for it to finish" };
  });
  const onInstalled = vi.fn();
  render(<SyntaxBadge info={{ language: "Rust", language_key: "rust", status: "broken" }} onInstalled={onInstalled} />);
  expect(screen.getByText("Plain text · Rust parser needs repair")).toBeTruthy();
  expect((await screen.findByRole("alert")).textContent).toContain("Rust parser install failed: cc: not found");
  fireEvent.click(screen.getByRole("button", { name: "Repair Rust parser…" }));
  fireEvent.click(screen.getByRole("button", { name: "Repair parser" }));
  expect(await screen.findByText(/Go parser is already being installed/)).toBeTruthy();
  expect(onInstalled).not.toHaveBeenCalled();
});

const file: DiffFile = { path: "src/lib.rs", old_path: null, status: "modified", additions: 2, deletions: 1, is_binary: false, patch: "", hunks: [hunk], syntax: rust };
const diffView: DiffView = {
  target: { project_id: "p", feature_id: "f" }, feature_name: "Feature", branch: "feature", base_ref: "main", base_commit: "base123456",
  commit: null, commits: [], commits_error: null, total_additions: 2, total_deletions: 1,
  files: [{ ...file, syntax: { language: "Python", language_key: "python", status: "not_installed" }, path: "tool.py",
    hunks: [{ header: "@@ -1 +1 @@", lines: [{ kind: "added", text: "+print(1)", old_line: null, new_line: 1, syntax: null }] }] }, file],
};

it("refetches the diff after an install so the file gains colours", async () => {
  let status = idle;
  let loads = 0;
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "load_diff") {
      loads += 1;
      return loads === 1 ? diffView : { ...diffView, files: [{ ...diffView.files[0], syntax: { ...diffView.files[0].syntax!, status: "highlighted" },
        hunks: [{ header: "@@ -1 +1 @@", lines: [{ kind: "added", text: "+print(1)", old_line: null, new_line: 1, syntax: [["", "+"], ["function-builtin", "print"], ["", "("], ["number", "1"], ["", ")"]] }] }] }, file] };
    }
    if (command === "syntax_install_status") return status;
    status = { ...idle, language: "Python", language_key: "python", running: true };
    return status;
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  render(<QueryClientProvider client={client}><DiffPanel target={diffView.target} onClose={vi.fn()} /></QueryClientProvider>);
  fireEvent.click(await screen.findByRole("button", { name: "Install Python parser…" }));
  fireEvent.click(screen.getByRole("button", { name: "Install parser" }));
  await screen.findByText(/Installing the Python parser/);
  await act(async () => { status = { ...status, running: false, message: "Installed Python tree-sitter parser", completed: 1 }; });
  await waitFor(() => expect(codeCell("+print(1)")?.querySelector(".syn-function-builtin")?.textContent).toBe("print"), { timeout: 3000 });
  expect(loads).toBe(2);
  expect(screen.getByText("Python").className).toContain("syntax-chip-on");
  // The Rust file was already highlighted.
  fireEvent.click(screen.getByRole("button", { name: /src\/lib.rs modified/ }));
  expect(screen.getByText("Rust").className).toContain("syntax-chip-on");
  expect(codeCell("-   still a comment */").querySelector(".syn-comment")).toBeTruthy();
});

const reviewView: ReviewView = {
  workflow_id: "review", revision: 2, target: { project_id: "p", feature_id: "f" },
  feature_name: "Feature", branch: "feature", base_ref: "main", selected_path: "src/lib.rs",
  general_feedback: "", has_prior_review: false, error: null, save_error: null, applied_suggestions: [], history: null, summary: null, check_command: null, check: null, finish: null,
  ai: { precall: null, running: false, walkthrough_path: null, co_review_path: null, overview_running: false, overview: null, question_running: false, questions: [], question_error: null, comment_draft: null, ready_comment: null, harnesses: ["claude"], message: null },
  files: [{ diff: file, verdict: "undecided", feedback: "", severity: "suggestion", changed_since_last: false, notes: null, walkthrough: null, comment: null, line_comments: [] }],
};

it("comments on a highlighted review line through the existing span anchors", async () => {
  const onAct = vi.fn(async () => true);
  render(<ReviewPanel view={reviewView} busy={false} error={null} onAct={onAct} />);
  expect(screen.getByText("Rust").className).toContain("syntax-chip-on");
  expect(codeCell("+let s = \"multi").querySelector(".syn-string")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Select line 5", exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "Comment on selection" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Line comment" }), { target: { value: "Unterminated string" } });
  fireEvent.click(screen.getByRole("button", { name: "Save comment" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "line_comment", path: "src/lib.rs",
    start: { old_line: null, new_line: 5 }, end: { old_line: null, new_line: 5 }, text: "Unterminated string", severity: "suggestion" }));
});

const learning: LearningView = {
  workflow_id: "reader", revision: 0, target: { project_id: "p", feature_id: "f" }, feature_name: "Feature", scope: "repo_tree", is_git: true,
  entries: [{ key: "file:src/lib.rs", label: "src/lib.rs", kind: "file", depth: 0, expanded: false }],
  content_path: "src/lib.rs", content: ["/* opens", "   closes */", "", "fn main() {}"], content_line_labels: ["1", "2", "3", "4"],
  content_syntax: [[["comment", "/* opens"]], [["comment", "   closes */"]], null, [["keyword", "fn"], ["", " "], ["function", "main"], ["punctuation-bracket", "() {}"]]],
  syntax: rust, content_error: null, anchor: "all of src/lib.rs", selection: [2, 2], hunks: [], starters: [], can_keep_todo: true,
  harness: "claude", harnesses: ["claude"], level: "newcomer", history_saved: true, error: null, notice: null, qa: [],
};

it("colours the Learning reader and keeps its line selection", () => {
  const onAct = vi.fn(async () => true);
  render(<LearningPanel view={learning} busy={false} onAct={onAct} onClose={vi.fn()} onLaunch={vi.fn()} />);
  expect(screen.getByText("Rust").className).toContain("syntax-chip-on");
  const second = screen.getByRole("button", { name: "Select line 2" });
  expect(second.className).toContain("learning-line-selected");
  expect(second.querySelector(".syn-comment")?.textContent).toBe("   closes */");
  expect(screen.getByRole("button", { name: "Select line 3" }).querySelector("code")?.textContent).toBe(" ");
  expect(screen.getByRole("button", { name: "Select line 4" }).querySelector(".syn-function")?.textContent).toBe("main");
  fireEvent.click(screen.getByRole("button", { name: "Select line 4" }));
  expect(onAct).toHaveBeenLastCalledWith({ kind: "lines_anchor", start: 4, end: 4 });
});
