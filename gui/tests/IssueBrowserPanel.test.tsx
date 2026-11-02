// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import IssueBrowserPanel, { matchesIssue } from "../src/IssueBrowserPanel";
import type { IssueFixRequest, IssueListView, IssuePreview } from "../src/issueApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

const agents = [{ slug: "claude", display_name: "Claude Code" }] as never;
const modes = [{ slug: "vibeless", display_name: "Vibeless", description: "Ask first" }] as never;

const list: IssueListView = {
  project_id: "p1", project_name: "demo", repository: "github.com/acme/widget", page: 1, per_page: 25, has_next_page: true,
  issues: [
    { number: 7, title: "Crash on save", labels: ["bug"], updated_at: "2026-10-30", url: "u7", excerpt: "Stack trace here", linked_features: [] },
    { number: 8, title: "Docs typo", labels: ["docs", "good first issue"], updated_at: "2026-10-29", url: "u8", excerpt: "Fix the readme",
      linked_features: [{ project_name: "demo", feature_name: "fix-issue-8" }] },
  ],
};
const preview = (over: Partial<IssuePreview> = {}): IssuePreview => ({
  project_id: "p1", project_name: "demo", repository: "github.com/acme/widget", number: 7, title: "Crash on save",
  labels: ["bug"], updated_at: "2026-10-30", url: "u7", body: "Stack trace here", body_truncated: false, digest: "d".repeat(64),
  prompt: "Fix GitHub issue #7", suggested_feature_name: "fix-issue-7", suggested_branch: "issue/7-crash-on-save",
  linked_features: [], ...over,
});
const created = {
  project_id: "p1", feature_id: "f1", feature_name: "fix-issue-7", branch: "issue/7-crash-on-save", started: false,
  session: null, draft_prompt: "Fix GitHub issue #7", message: "Created 'fix-issue-7'",
};

type Handlers = Partial<Record<string, (args: Record<string, unknown>) => unknown>>;
function backend(handlers: Handlers) {
  vi.mocked(invoke).mockImplementation(async (command: string, args?: unknown) => {
    if (command === "worktree_hook_prompt") return null;
    const handler = handlers[command];
    if (!handler) throw new Error(`unexpected command ${command}`);
    return handler((args ?? {}) as Record<string, unknown>);
  });
}
const calls = (command: string) => vi.mocked(invoke).mock.calls.filter(([name]) => name === command);

function mount(onCreated = vi.fn(), onClose = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <IssueBrowserPanel projectId="p1" projectName="demo" agents={agents} modes={modes} isGit onClose={onClose} onCreated={onCreated} />
    </QueryClientProvider>,
  );
  return { onCreated, onClose };
}

async function openIssue(number: string) {
  fireEvent.click(await screen.findByRole("button", { name: new RegExp(`Open issue #${number}`) }));
  await screen.findByRole("region", { name: "Issue preview" });
}

it("matches by number, title, label and excerpt, narrowed by label", () => {
  const row = list.issues[1];
  expect(matchesIssue(row, "#8", "")).toBe(true);
  expect(matchesIssue(row, "typo", "")).toBe(true);
  expect(matchesIssue(row, "readme", "")).toBe(true);
  expect(matchesIssue(row, "first", "")).toBe(true);
  expect(matchesIssue(row, "", "bug")).toBe(false);
  expect(matchesIssue(row, "typo", "docs")).toBe(true);
  expect(matchesIssue(row, "9", "")).toBe(false);
});

it("lists issues and filters them locally without another GitHub call", async () => {
  backend({ issue_browser_list: () => list });
  mount();
  await screen.findByRole("button", { name: /Open issue #7/ });
  expect(screen.getByText("Feature exists: fix-issue-8")).toBeTruthy();

  fireEvent.change(screen.getByLabelText("Search issues"), { target: { value: "readme" } });
  expect(screen.queryByRole("button", { name: /Open issue #7/ })).toBeNull();
  expect(screen.getByRole("button", { name: /Open issue #8/ })).toBeTruthy();

  fireEvent.change(screen.getByLabelText("Search issues"), { target: { value: "" } });
  fireEvent.change(screen.getByLabelText("Filter by label"), { target: { value: "bug" } });
  expect(screen.queryByRole("button", { name: /Open issue #8/ })).toBeNull();
  expect(calls("issue_browser_list")).toHaveLength(1);
});

it("pages with Next and states an explicit reason with a retry when GitHub fails", async () => {
  let fail = true;
  backend({
    issue_browser_list: (args) => {
      if (fail) throw { kind: "conflict", message: "The GitHub CLI (`gh`) is not installed or not on PATH." };
      return { ...list, page: args.page as number };
    },
  });
  mount();
  expect((await screen.findByRole("alert")).textContent).toContain("not installed");
  fail = false;
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await screen.findByRole("button", { name: /Open issue #7/ });
  fireEvent.click(screen.getByRole("button", { name: "Next" }));
  await screen.findByText("Page 2");
  expect(calls("issue_browser_list").map(([, args]) => (args as { page: number }).page)).toEqual([1, 1, 2]);
});

it("previews an issue and creates it stopped, sending the preview digest and no GitHub write", async () => {
  backend({ issue_browser_list: () => list, issue_fix_preview: () => preview(), issue_fix_create: () => created });
  const { onCreated } = mount();
  await openIssue("7");
  expect(screen.getByText("Stack trace here")).toBeTruthy();
  expect((screen.getByLabelText("Agent prompt") as HTMLTextAreaElement).value).toBe("Fix GitHub issue #7");

  const create = await screen.findByRole("button", { name: "Create feature" });
  await waitFor(() => expect((create as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(create);
  await waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));

  const sent = (calls("issue_fix_create")[0][1] as { request: IssueFixRequest }).request;
  expect(sent).toMatchObject({
    project_id: "p1", number: 7, digest: "d".repeat(64), feature_name: "fix-issue-7", start: false, approved: false,
    allow_duplicate: false, prompt: "Fix GitHub issue #7",
  });
  expect(sent.request).toMatchObject({ branch: "issue/7-crash-on-save", agent: "claude", mode: "vibeless", plan_mode: false, use_worktree: true });
  const writes = vi.mocked(invoke).mock.calls.map(([name]) => name).filter((name) => /comment|post|write/i.test(String(name)));
  expect(writes).toEqual([]);
});

it("refuses a changed issue, shows why and re-previews on Reload", async () => {
  let previews = 0;
  backend({
    issue_browser_list: () => list,
    issue_fix_preview: () => preview(previews++ === 0 ? {} : { digest: "e".repeat(64), body: "New text" }),
    issue_fix_create: () => { throw { kind: "conflict", message: "Issue #7 changed on GitHub since you previewed it. Reload the preview." }; },
  });
  const { onCreated } = mount();
  await openIssue("7");
  const create = await screen.findByRole("button", { name: "Create feature" });
  await waitFor(() => expect((create as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(create);
  await screen.findByText(/changed on GitHub/);
  expect(onCreated).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Reload issue" }));
  await screen.findByText("New text");
  expect(calls("issue_fix_preview")).toHaveLength(2);
});

it("needs explicit confirmation to duplicate a feature and to exceed resource limits", async () => {
  const requests: IssueFixRequest[] = [];
  backend({
    issue_browser_list: () => list,
    issue_fix_preview: () => preview({ number: 8, linked_features: [{ project_name: "demo", feature_name: "fix-issue-8" }] }),
    issue_fix_create: (args) => {
      const request = args.request as IssueFixRequest;
      requests.push(request);
      if (request.start && !request.approved) throw { kind: "needs_approval", message: "Starting an agent would exceed the limit." };
      return created;
    },
  });
  const { onCreated } = mount();
  await openIssue("8");
  const create = await screen.findByRole("button", { name: "Create feature" });
  expect((create as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByLabelText("Create another feature for this issue"));
  fireEvent.click(screen.getByLabelText(/^Start the agent now/));
  const start = await screen.findByRole("button", { name: "Create and start" });
  await waitFor(() => expect((start as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(start);
  fireEvent.click(await screen.findByRole("button", { name: "Start anyway" }));
  await waitFor(() => expect(onCreated).toHaveBeenCalled());
  expect(requests.map((r) => [r.start, r.approved, r.allow_duplicate])).toEqual([[true, false, true], [true, true, true]]);
});
