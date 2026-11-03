// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import GithubProblemNotice from "../src/GithubProblemNotice";
import PrTriagePanel from "../src/PrTriagePanel";
import PrReviewPanel from "../src/PrReviewPanel";
import type { GithubProblem } from "../src/api";
import type { PrTriageAction, PrTriageView } from "../src/prTriageApi";
import type { PrReviewAction, PrReviewView } from "../src/prReviewApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

const notSignedIn: GithubProblem = {
  kind: "not_signed_in", title: "The GitHub CLI (gh) is not signed in",
  message: "Pull requests are read with the GitHub CLI (gh). It is installed but not signed in to GitHub. Run this in a terminal, then press Retry.",
  command: "gh auth login", link: null, detail: "`gh pr list` failed: To get started with GitHub CLI, please run:  gh auth login",
};
const cliMissing: GithubProblem = {
  kind: "cli_missing", title: "The GitHub CLI (gh) is not installed",
  message: "Pull requests are read with the GitHub CLI (gh), and AMF could not run it. Install it from cli.github.com, then press Retry.",
  command: null, link: "https://cli.github.com", detail: "Failed to run `gh pr list`.: No such file or directory (os error 2)",
};

it("names the problem, the command to run and keeps the raw text under Details", () => {
  const onRetry = vi.fn();
  render(<GithubProblemNotice problem={notSignedIn} onRetry={onRetry} />);
  const alert = screen.getByRole("alert");
  expect(within(alert).getByText("The GitHub CLI (gh) is not signed in")).toBeTruthy();
  expect(within(alert).getByLabelText("Command to run in a terminal").textContent).toBe("gh auth login");
  expect(within(alert).getByText("Details")).toBeTruthy();
  expect(alert.textContent).toContain("please run:  gh auth login");
  fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
  expect(onRetry).toHaveBeenCalledOnce();
});

it("opens the install link in the system browser and offers no Retry without a handler", async () => {
  vi.mocked(invoke).mockResolvedValue(undefined);
  render(<GithubProblemNotice problem={cliMissing} />);
  expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  expect(screen.queryByLabelText("Command to run in a terminal")).toBeNull();
  fireEvent.click(screen.getByRole("link", { name: "cli.github.com" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("screenshots_open_browser", { url: "https://cli.github.com" }));
});

const target = { project_id: "project", feature_id: "feature" };
const triagePick: PrTriageView = {
  workflow_id: "triage", revision: 1, target, feature_name: "Round totals", branch: "round-totals",
  stage: "pick", loading_pr: null, review: null, precall: null, reply: null, write_confirm: null,
  fix_targets: [], fix_draft: null, handoff: null, harnesses: ["claude"], default_harness: "claude", error: null, notice: null,
  picker: { include_closed: false, error: notSignedIn.detail, github_problem: notSignedIn, branch_pr: null, loading: false, entries: [] },
};

it("PR Triage shows a picker sign-in problem in plain words and Retry re-reads the list", async () => {
  const listed: PrTriageView = { ...triagePick, revision: 2, picker: { ...triagePick.picker!, error: null, github_problem: null, entries: [
    { number: 12, title: "Round invoice totals", author: "dev", head_ref: "round-totals", updated_at: "", is_draft: false, state: "OPEN", mine: true, selected_for_feature: true },
  ] } };
  const acted: PrTriageAction[] = [];
  vi.mocked(invoke).mockImplementation(async (command: string, args?: unknown) => {
    if (command === "pr_triage_begin") return triagePick;
    if (command === "pr_triage_act") { acted.push((args as { action: PrTriageAction }).action); return listed; }
    throw new Error(command);
  });
  render(<PrTriagePanel target={target} onClose={vi.fn()} />);
  expect(await screen.findByText("The GitHub CLI (gh) is not signed in")).toBeTruthy();
  // The raw `gh` text is not the headline.
  expect(screen.queryByText(/Could not list pull requests:/)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await screen.findByRole("button", { name: /Open PR #12/ });
  expect(acted).toEqual([{ kind: "refresh" }]);
  expect(screen.queryByText("The GitHub CLI (gh) is not signed in")).toBeNull();
});

it("PR Triage picker has its own Refresh, and explains an unrecognised list failure", async () => {
  const failed: PrTriageView = { ...triagePick, picker: { ...triagePick.picker!, error: "`gh pr list` failed: something odd", github_problem: null } };
  vi.mocked(invoke).mockImplementation(async (command: string) => {
    if (command === "pr_triage_begin") return failed;
    if (command === "pr_triage_act") return failed;
    throw new Error(command);
  });
  render(<PrTriagePanel target={target} onClose={vi.fn()} />);
  expect(await screen.findByText("Could not list pull requests")).toBeTruthy();
  expect(screen.getByText("gh auth login")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await waitFor(() => expect(vi.mocked(invoke).mock.calls.some(([command, args]) =>
    command === "pr_triage_act" && (args as { action: PrTriageAction }).action.kind === "refresh")).toBe(true));
});

it("PR Triage that cannot open at all because gh is missing retries the open", async () => {
  let begins = 0;
  vi.mocked(invoke).mockImplementation(async (command: string) => {
    if (command === "pr_triage_begin") {
      if (++begins === 1) throw { kind: "internal", message: `${cliMissing.title} ${cliMissing.message}`, github: cliMissing };
      return { ...triagePick, picker: { ...triagePick.picker!, error: null, github_problem: null } };
    }
    throw new Error(command);
  });
  render(<PrTriagePanel target={target} onClose={vi.fn()} />);
  expect(await screen.findByText("The GitHub CLI (gh) is not installed")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() => expect(begins).toBe(2));
  await waitFor(() => expect(screen.queryByText("The GitHub CLI (gh) is not installed")).toBeNull());
});

it("PR Review shows a list failure as its fix and Retry re-reads the list", async () => {
  const failed: PrReviewView = {
    workflow_id: "review", revision: 1, project_name: "Demo", head_sha: null, stage: "pick", loading: false, entries: [],
    number: null, title: null, files: [], summary: "", submission: null, error: notSignedIn.detail, github_problem: notSignedIn, notice: null,
  };
  const acted: PrReviewAction[] = [];
  vi.mocked(invoke).mockImplementation(async (command: string, args?: unknown) => {
    if (command === "pr_review_begin") return failed;
    if (command === "pr_review_act") { acted.push((args as { action: PrReviewAction }).action); return { ...failed, revision: 2, error: null, github_problem: null }; }
    throw new Error(command);
  });
  render(<PrReviewPanel projectId="project" onClose={vi.fn()} />);
  expect(await screen.findByText("The GitHub CLI (gh) is not signed in")).toBeTruthy();
  expect(screen.getByLabelText("Command to run in a terminal").textContent).toBe("gh auth login");
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() => expect(acted).toEqual([{ kind: "retry" }]));
  await waitFor(() => expect(screen.queryByText("The GitHub CLI (gh) is not signed in")).toBeNull());
});
