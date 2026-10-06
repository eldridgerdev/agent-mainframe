// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import SidebarTree, { featureGlyph, prLabel, SidebarTreeProps, withCollapsed } from "../src/SidebarTree";
import type { Feature, FeatureSession, Project, SidebarFeature, SidebarSnapshot, WorkspaceSnapshot } from "../src/api";

afterEach(cleanup);

function session(id: string, kind: string, extra: Partial<FeatureSession> = {}): FeatureSession {
  return { id, kind, label: id, tmux_window: id, ...extra };
}

function feature(id: string, extra: Partial<Feature> = {}): Feature {
  return {
    id, name: id, branch: id, workdir: `/repo/.worktrees/${id}`, is_worktree: true, status: "idle",
    agent: "claude", mode: "vibeless", sessions: [], collapsed: true, ready: false, ...extra,
  };
}

function derived(extra: Partial<SidebarFeature> = {}): SidebarFeature {
  return {
    workdir_display: "~/repo/.worktrees/x", created_age: "5m ago", issue: null, summary_age: null, usage: null,
    pr: null, thinking: false, waiting_for_input: false, pending_input: false, ...extra,
  };
}

function renderTree(projects: Project[], sidebar: Partial<SidebarSnapshot> = {}, props: Partial<SidebarTreeProps> = {}) {
  const handlers = {
    onSelectProject: vi.fn(), onSelectFeature: vi.fn(), onSelectSession: vi.fn(),
    onToggleCollapsed: vi.fn(), onResumePlan: vi.fn(), onCreateFeature: vi.fn(),
  };
  render(<nav><SidebarTree
    projects={projects}
    sidebar={{ projects: {}, features: {}, sessions: {}, ...sidebar }}
    stoppedSessionIds={[]}
    selection={null}
    deletingFeatureId={null}
    pausedPlan={null}
    {...handlers}
    {...props}
  /></nav>);
  return handlers;
}

const project = (features: Feature[], extra: Partial<Project> = {}): Project =>
  ({ id: "p1", name: "Invoice API", repo: "/home/me/invoice", is_git: true, features, ...extra });

it("orders the status glyph exactly as the TUI tree does", () => {
  const all = derived({ thinking: true, waiting_for_input: true });
  const base = feature("f", { pending_worktree_script: true, ready: true, status: "active" });
  expect(featureGlyph(base, all, true)).toBe("worktree_script");
  expect(featureGlyph({ ...base, pending_worktree_script: false }, all, true)).toBe("deleting");
  const plain = { ...base, pending_worktree_script: false };
  expect(featureGlyph(plain, all, false)).toBe("waiting");
  expect(featureGlyph(plain, derived({ thinking: true }), false)).toBe("thinking");
  expect(featureGlyph(plain, derived(), false)).toBe("ready");
  expect(featureGlyph({ ...plain, ready: false }, derived(), false)).toBe("active");
  expect(featureGlyph({ ...plain, ready: false, status: "idle" }, undefined, false)).toBe("idle");
  // A stopped feature cannot be mid-turn, whatever a marker says.
  expect(featureGlyph({ ...plain, ready: false, status: "stopped" }, derived({ thinking: true }), false)).toBe("stopped");
});

it("labels PR badges like the TUI for open, unknown-thread, merged and closed PRs", () => {
  expect(prLabel({ state: "open", number: 321, unresolved_threads: 4 })).toBe("PR #321 · 4 open");
  expect(prLabel({ state: "open", number: 321, unresolved_threads: 0 })).toBe("PR #321 · 0 open");
  expect(prLabel({ state: "open", number: 321, unresolved_threads: null })).toBe("PR #321");
  expect(prLabel({ state: "merged", number: 7 })).toBe("PR #7 merged");
  expect(prLabel({ state: "closed", number: 8 })).toBe("PR #8 closed");
});

it("shows every feature badge and keeps the name as the row's accessible name", () => {
  const nick = feature("round-totals", {
    nickname: "Round totals", is_worktree: false, mode: "supervibe", review: true, plan_mode: true,
    remote_control: true, pending_worktree_script: true, summary: "Rounds invoice totals to cents",
    sessions: [session("Claude 1", "claude"), session("Shell", "terminal", { stopped: true })],
  });
  renderTree([project([nick])], {
    features: { "round-totals": derived({
      issue: "github.com/acme/invoice#42", usage: "usage 21.8k eff · $0.07", pending_input: true,
      waiting_for_input: true, pr: { state: "open", number: 321, unresolved_threads: 4 },
    }) },
  }, { pausedPlan: { featureId: "round-totals", projectName: null, featureName: "Round totals" }, deletingFeatureId: "round-totals" });

  const name = screen.getByRole("button", { name: "Round totals", exact: true });
  const row = name.closest(".tree-feature") as HTMLElement;
  expect(within(row).getByText("(round-totals)")).toBeTruthy();
  const meta = document.getElementById(name.getAttribute("aria-describedby")!)!;
  const chips = Array.from(meta.querySelectorAll(".tree-chip")).map((chip) => chip.textContent);
  expect(chips).toEqual([
    "repo", "github.com/acme/invoice#42", "PR #321 · 4 open", "usage 21.8k eff · $0.07", "supervibe", "review",
    "plan", "plan paused · Resume", "remote", "deleting…", "running worktree script…", "5m ago", "2 sessions", "1 stopped",
  ]);
  expect(meta.querySelector(".mode-supervibe")).toBeTruthy();
  expect(meta.querySelector(".tree-pr.pr-open")).toBeTruthy();
  expect(within(row).getByRole("img", { name: "Input requested" })).toBeTruthy();
  // The worktree script outranks deletion and the waiting request.
  expect(within(row).getByRole("img", { name: "Running worktree script" })).toBeTruthy();
  expect(within(row).getByText("— Rounds invoice totals to cents")).toBeTruthy();
  expect(name.className).toContain("tree-name-deleting");
});

it("colours PR states and omits optional badges that do not apply", () => {
  renderTree([project([feature("a"), feature("b"), feature("c")])], {
    features: {
      a: derived({ pr: { state: "open", number: 1, unresolved_threads: 0 } }),
      b: derived({ pr: { state: "merged", number: 2 } }),
      c: derived({ pr: { state: "closed", number: 3 } }),
    },
  });
  expect(screen.getByText("PR #1 · 0 open").className).toContain("pr-clear");
  expect(screen.getByText("PR #2 merged").className).toContain("pr-merged");
  expect(screen.getByText("PR #3 closed").className).toContain("pr-closed");
  expect(screen.queryByText("repo")).toBeNull();
  expect(screen.queryByText("review")).toBeNull();
  expect(screen.queryByRole("img", { name: "Input requested" })).toBeNull();
});

it("marks a waiting diff review with the waiting glyph but not the input marker", () => {
  renderTree([project([feature("a")])], { features: { a: derived({ waiting_for_input: true }) } });
  expect(screen.getByRole("img", { name: "Waiting for input" })).toBeTruthy();
  expect(screen.queryByRole("img", { name: "Input requested" })).toBeNull();
});

it("shows the project path, an empty-project hint and a paused creation-time plan", () => {
  const handlers = renderTree([project([], { id: "p2", name: "Empty" })], {
    projects: { p2: { repo_display: "~/invoice" } },
  }, { pausedPlan: { featureId: null, projectName: "Empty", featureName: "planned-feature" } });
  expect(screen.getByText("~/invoice")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "No features yet. Add one" }));
  expect(handlers.onCreateFeature).toHaveBeenCalledWith("p2");
  fireEvent.click(screen.getByRole("button", { name: "plan paused: planned-feature · Resume" }));
  expect(handlers.onResumePlan).toHaveBeenCalledTimes(1);
});

it("nests session rows with kind icons, run state, context and status text", () => {
  const sessions = [
    session("Claude 1", "claude"),
    session("Codex 1", "codex", { stopped: true }),
    session("Shell", "terminal"),
    session("Editor", "nvim"),
    session("Code", "vscode"),
    session("Dev server", "custom"),
    session("TODOs", "todos"),
  ];
  const handlers = renderTree([project([feature("f", { status: "active", collapsed: false, sessions })])], {
    features: { f: derived({ workdir_display: "~/repo/.worktrees/f" }) },
    sessions: {
      "Claude 1": { status_text: "usage 21.8k eff", context: { text: "Ctx 64%", band: "normal", stale: false, pending_reset: false }, icon: null, icon_nerd: null },
      "Codex 1": { status_text: null, context: { text: "Ctx ~91% CRITICAL STALE", band: "critical", stale: true, pending_reset: false }, icon: null, icon_nerd: null },
      Shell: { status_text: null, context: null, icon: null, icon_nerd: null },
      "Dev server": { status_text: "listening on :3000", context: null, icon: "S", icon_nerd: null },
    },
  }, { stoppedSessionIds: ["Shell"] });

  expect(screen.getByText("~/repo/.worktrees/f")).toBeTruthy();
  const row = (label: string) => screen.getByText(label, { selector: ".tree-session-label" }).closest(".tree-session") as HTMLElement;
  const icon = (label: string) => row(label).querySelector(".tree-kind")!.textContent;
  expect(icon("Claude 1")).toBe("*");
  expect(icon("Shell")).toBe(">");
  expect(icon("Editor")).toBe("");
  expect(icon("Code")).toBe("");
  expect(icon("Dev server")).toBe("S");
  expect(icon("TODOs")).toBe("");
  const dot = (label: string) => row(label).querySelector(".status-dot")?.className;
  expect(dot("Claude 1")).toContain("status-active");
  expect(dot("Codex 1")).toContain("status-stopped");
  expect(dot("Shell")).toContain("status-stopped");
  expect(dot("TODOs")).toBeUndefined();
  expect(row("Codex 1").querySelector(".tree-session-stopped")).toBeTruthy();
  expect(row("Claude 1").querySelector(".ctx-normal")!.textContent).toBe("Ctx 64%");
  const critical = row("Codex 1").querySelector(".tree-context")!;
  expect(critical.className).toContain("ctx-critical");
  expect(critical.className).toContain("ctx-stale");
  expect(within(row("Claude 1")).getByText("usage 21.8k eff")).toBeTruthy();
  expect(within(row("Dev server")).getByText("listening on :3000")).toBeTruthy();
  expect(row("Shell").querySelector(".tree-context")).toBeNull();

  fireEvent.click(within(row("Codex 1")).getByRole("button"));
  expect(handlers.onSelectSession).toHaveBeenCalledWith({ project_id: "p1", feature_id: "f", session_id: "Codex 1" }, "codex");
  fireEvent.click(within(row("TODOs")).getByRole("button"));
  expect(handlers.onSelectSession).toHaveBeenLastCalledWith({ project_id: "p1", feature_id: "f", session_id: "TODOs" }, "todos");
});

it("truncates a long session label before the context indicator and says when a reset is pending", () => {
  const long = "A very long OpenCode session name that cannot fit";
  renderTree([project([feature("f", { collapsed: false, sessions: [session(long, "opencode"), session("Pi", "pi")] })])], {
    sessions: {
      [long]: { status_text: null, context: { text: "Ctx ~91% CRITICAL · 819,000", band: "critical", stale: false, pending_reset: false }, icon: null, icon_nerd: null },
      Pi: { status_text: null, context: { text: "", band: "normal", stale: false, pending_reset: true }, icon: null, icon_nerd: null },
    },
  });
  const label = screen.getByText(long, { selector: ".tree-session-label" });
  const button = label.closest("button")!;
  // The indicator is a sibling after the (ellipsising) label, never inside it.
  // The row keeps percentage and band; the token count is in the tooltip.
  expect(label.nextElementSibling?.textContent).toBe("Ctx ~91% CRITICAL");
  expect(label.nextElementSibling?.getAttribute("title")).toBe("Ctx ~91% CRITICAL · 819,000");
  expect(button.getAttribute("title")).toContain(long);
  expect(button.getAttribute("title")).toContain("Ctx ~91% CRITICAL · 819,000");
  expect(screen.getByText("Ctx resetting").className).toContain("ctx-pending");
  expect(screen.queryByText(/Ctx \d+%/, { selector: ".ctx-pending" })).toBeNull();
});

it("toggles persisted collapse by stable ids and hides rows by the shared flags", () => {
  const features = [feature("f", { collapsed: true, sessions: [session("Claude 1", "claude")] }), feature("g")];
  const handlers = renderTree([project(features)]);
  // A feature collapsed in the store shows no session rows; one with no
  // sessions has nothing to expand.
  expect(screen.queryByText("Claude 1")).toBeNull();
  expect(screen.queryByRole("button", { name: "Show sessions of g" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Show sessions of f" }));
  expect(handlers.onToggleCollapsed).toHaveBeenCalledWith({ project_id: "p1", feature_id: "f" }, false);
  fireEvent.click(screen.getByRole("button", { name: "Collapse Invoice API" }));
  expect(handlers.onToggleCollapsed).toHaveBeenLastCalledWith({ project_id: "p1", feature_id: null }, true);

  cleanup();
  renderTree([project(features, { collapsed: true })]);
  expect(screen.queryByRole("button", { name: "f", exact: true })).toBeNull();
  expect(screen.getByRole("button", { name: "Expand Invoice API" }).getAttribute("aria-expanded")).toBe("false");
});

it("applies a collapse change to a snapshot without touching other rows", () => {
  const snapshot: WorkspaceSnapshot = {
    projects: [project([feature("f"), feature("g", { collapsed: false })]), project([], { id: "p2" })],
    snapshot_at: "", stopped_session_ids: [],
  };
  const projectCollapsed = withCollapsed(snapshot, { project_id: "p1", feature_id: null }, true);
  expect(projectCollapsed.projects[0].collapsed).toBe(true);
  expect(projectCollapsed.projects[1]).toBe(snapshot.projects[1]);
  const featureOpen = withCollapsed(snapshot, { project_id: "p1", feature_id: "f" }, false);
  expect(featureOpen.projects[0].features.map((f) => f.collapsed)).toEqual([false, false]);
  expect(featureOpen.projects[0].features[1]).toBe(snapshot.projects[0].features[1]);
});

it("renders from the persisted snapshot alone and carries a workflow's extra marker", () => {
  render(<nav><SidebarTree
    projects={[project([feature("f", { status: "stopped" })])]}
    sidebar={undefined}
    stoppedSessionIds={[]}
    selection={{ kind: "feature", projectId: "p1", featureId: "f", tab: undefined }}
    deletingFeatureId={null}
    pausedPlan={null}
    onSelectProject={vi.fn()} onSelectFeature={vi.fn()} onSelectSession={vi.fn()}
    onToggleCollapsed={vi.fn()} onResumePlan={vi.fn()} onCreateFeature={vi.fn()}
    renderFeatureExtra={(f) => <span className="nav-count">{`${f.id}-extra`}</span>}
  /></nav>);
  const name = screen.getByRole("button", { name: "f", exact: true });
  expect(name.getAttribute("aria-current")).toBe("page");
  expect(screen.getByRole("img", { name: "Stopped" })).toBeTruthy();
  expect(screen.getByText("f-extra")).toBeTruthy();
  expect(screen.getByText("/home/me/invoice")).toBeTruthy();
});
