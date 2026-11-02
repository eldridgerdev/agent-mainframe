// @vitest-environment jsdom
import { creationChoices } from "./creationChoices";
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import App, { CreateFeatureForm } from "../src/App";
import SteeringPanel from "../src/SteeringPanel";
import type { SteeringAnalysis } from "../src/steeringApi";
import type { WorkspaceSnapshot } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => vi.fn()) }));
vi.mock("../src/TerminalPane", async () => {
  const { useEffect } = await import("react");
  return { default: ({ onReadyChange }: { onReadyChange: (ready: boolean) => void }) => {
    useEffect(() => { onReadyChange(true); }, [onReadyChange]);
    return null;
  } };
});
vi.mock("../src/TodoPanel", () => ({ default: () => null }));
afterEach(() => { cleanup(); vi.clearAllMocks(); localStorage.clear(); });

/** A stand-in for the pure backend scan: digest is the text itself. */
function analysis(text: string): SteeringAnalysis {
  const covered = text.includes("cargo test");
  return {
    digest: `digest:${text}`, score: covered ? 2 : 0, max_score: 10, blank: !text.trim(),
    summary: covered ? "Thin draft. Add missing constraints now." : "Start with the concrete task.",
    checks: [
      { key: "file_scope", label: "File scope", present: false, detail: "The prompt does not clearly name what files or directories are in scope." },
      { key: "validation_commands", label: "Validation commands", present: covered, detail: covered ? "covered" : "The prompt does not tell the agent how to verify the change." },
    ],
    tips: covered ? ["Add a line like: `Touch only src/app/... and src/ui/...`"] : [],
  };
}
const calls = (name: string) => vi.mocked(invoke).mock.calls.filter(([command]) => command === name);
function backend(extra: (command: string, args: any) => unknown = () => undefined) {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "steering_analyze") return analysis((args as { text: string }).text);
    const result = extra(command, args);
    if (result === undefined) throw new Error(`Unexpected command: ${command}`);
    return result as never;
  });
}

it("shows the checklist for the draft and goes stale the moment the draft is edited", async () => {
  backend();
  const view = (text: string) => <SteeringPanel text={text} onClose={vi.fn()} />;
  const { rerender } = render(view("cargo test please"));
  expect(await screen.findByText("Score: 2 / 10")).toBeTruthy();
  const panel = screen.getByRole("region", { name: "Steering coach" });
  expect(panel.getAttribute("data-stale")).toBe("false");
  expect(panel.getAttribute("data-digest")).toBe("digest:cargo test please");
  expect(within(panel).getByText(/does not clearly name what files/)).toBeTruthy();
  expect(within(panel).getByRole("img", { name: "covered" })).toBeTruthy();
  expect(within(panel).getByText(/Add a line like/)).toBeTruthy();
  expect(calls("steering_analyze")).toHaveLength(1);

  rerender(view("cargo test please, and more"));
  expect(panel.getAttribute("data-stale")).toBe("true");
  expect(screen.getByRole("status").textContent).toContain("draft changed since this check");
  expect(screen.queryByText("Score: 2 / 10")).toBeNull();
  // Staleness alone must not re-run anything: the user re-checks explicitly.
  expect(calls("steering_analyze")).toHaveLength(1);

  fireEvent.click(screen.getByRole("button", { name: "Re-check draft" }));
  await waitFor(() => expect(panel.getAttribute("data-stale")).toBe("false"));
  expect(panel.getAttribute("data-digest")).toBe("digest:cargo test please, and more");
  expect(calls("steering_analyze")[1]).toEqual(["steering_analyze", { text: "cargo test please, and more" }]);
});

it("ignores an analysis that returns after the draft moved on", async () => {
  let finishFirst!: (value: SteeringAnalysis) => void;
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { finishFirst = resolve as never; }));
  backend();
  const { rerender } = render(<SteeringPanel text="first" onClose={vi.fn()} />);
  rerender(<SteeringPanel text="second" onClose={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Check draft" }));
  await screen.findByText("Start with the concrete task.");
  await act(async () => finishFirst({ ...analysis("first"), summary: "OLD RESULT" }));
  expect(screen.queryByText("OLD RESULT")).toBeNull();
  expect(screen.getByRole("region", { name: "Steering coach" }).getAttribute("data-digest")).toBe("digest:second");
});

it("reports a refused analysis without inventing a checklist", async () => {
  vi.mocked(invoke).mockRejectedValue({ kind: "internal", message: "The draft is 300 KiB; too long" });
  render(<SteeringPanel text="x" onClose={vi.fn()} />);
  expect((await screen.findByRole("alert")).textContent).toContain("too long");
  expect(screen.queryByRole("list", { name: "Constraint checklist" })).toBeNull();
});

function form(onSubmit = vi.fn()) {
  vi.mocked(invoke).mockImplementation(async (command) => command === "project_harness_choices" ? creationChoices(["claude"]) : command === "feature_presets_choices" ? [] : null);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(<QueryClientProvider client={client}><CreateFeatureForm projectId="one" projectName="one" isGit={false} defaultUseWorktree={false}
    agents={[{ slug: "claude", display_name: "Claude" }]} modes={[{ slug: "vibe", display_name: "Vibe", description: "" }]}
    pending={false} onCancel={vi.fn()} onSubmit={onSubmit} /></QueryClientProvider>);
  return onSubmit;
}
async function fillAndCreate() {
  await waitFor(() => expect(screen.getByRole("combobox", { name: "Harness" })).toHaveProperty("disabled", false));
  fireEvent.change(screen.getByLabelText("Branch / feature name"), { target: { value: "coached" } });
  fireEvent.click(screen.getByRole("button", { name: "Create feature" }));
}

it("creation form: the coach is off by default and passes no option", async () => {
  const onSubmit = form();
  expect(screen.getByRole("checkbox", { name: /Prompt coach \(experimental\)/ })).toHaveProperty("checked", false);
  await fillAndCreate();
  expect(onSubmit).toHaveBeenCalledOnce();
  expect(onSubmit.mock.calls[0]).toHaveLength(2);
});

it("creation form: ticking the coach reports it, and planning turns it off", async () => {
  const onSubmit = form();
  const box = screen.getByRole("checkbox", { name: /Prompt coach \(experimental\)/ });
  fireEvent.click(box);
  expect(box).toHaveProperty("checked", true);
  fireEvent.click(screen.getByRole("radio", { name: "Quick Plan" }));
  expect(box).toHaveProperty("disabled", true);
  expect(box).toHaveProperty("checked", false);
  expect(screen.getByText(/Planning already shapes the startup prompt/)).toBeTruthy();
  fireEvent.click(screen.getByText("Start directly"));
  expect(box).toHaveProperty("checked", true);
  await fillAndCreate();
  expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ branch: "coached" }), "none", { steering: true });
});

const snapshot: WorkspaceSnapshot = {
  projects: [{
    id: "project", name: "demo", repo: "/demo", is_git: false,
    features: [{
      id: "feature", name: "my-feat", branch: "my-feat", workdir: "/demo", is_worktree: false, status: "idle",
      agent: "claude", mode: "vibeless", sessions: [{ id: "Agent", kind: "claude", label: "Agent", tmux_window: "Agent" }],
    }],
  }],
  snapshot_at: "2026-09-26T00:00:00Z", stopped_session_ids: [],
};
const draftInput = () => screen.getByRole("textbox", { name: "Draft prompt" }) as HTMLTextAreaElement;
const submits = () => calls("terminal_submit_prompt");
function app() {
  backend((command) => {
    switch (command) {
      case "get_snapshot": return snapshot;
      case "supported_harnesses": return [{ slug: "claude", display_name: "Claude" }];
      case "supported_modes": return [{ slug: "vibe", display_name: "Vibe", description: "" }];
      case "plan_snapshot": return { active: null, draft: null };
      case "project_harness_choices": return creationChoices(["claude"]);
      case "feature_presets_choices": return [];
      case "worktree_hook_prompt": return null;
      case "create_feature": return { ok: true, project_id: "project", feature_id: "feature", started: true, message: "created" };
      case "terminal_submit_prompt": return null;
    }
    return undefined;
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><App /></QueryClientProvider>);
  return client;
}

it("creating with the coach opens an empty, unsent draft with the checklist and never sends", async () => {
  const client = app();
  fireEvent.click(await screen.findByRole("button", { name: "my-feat", exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "demo", exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "New feature", exact: true }));
  fireEvent.click(await screen.findByRole("checkbox", { name: /Prompt coach \(experimental\)/ }));
  await fillAndCreate();
  const panel = await screen.findByRole("region", { name: "Steering coach" });
  await within(panel).findByText("Start with the concrete task.");
  expect(draftInput().value).toBe("");
  expect(calls("create_feature")).toHaveLength(1);
  expect(submits()).toHaveLength(0);
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "terminal_input")).toBe(false);
  client.clear();
});

it("without the coach, creating leaves the composer plain", async () => {
  const client = app();
  fireEvent.click(await screen.findByRole("button", { name: "my-feat", exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "demo", exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "New feature", exact: true }));
  await screen.findByRole("checkbox", { name: /Prompt coach/ });
  await fillAndCreate();
  await screen.findByRole("textbox", { name: "Draft prompt" });
  expect(screen.queryByRole("region", { name: "Steering coach" })).toBeNull();
  expect(calls("steering_analyze")).toHaveLength(0);
  client.clear();
});

it("composer coach: stale after an edit, never sends, and closes after the explicit send", async () => {
  const client = app();
  fireEvent.click(await screen.findByRole("button", { name: "my-feat", exact: true }));
  fireEvent.change(await screen.findByRole("textbox", { name: "Draft prompt" }), { target: { value: "Fix it. cargo test" } });
  fireEvent.click(screen.getByRole("button", { name: "Steering coach" }));
  const panel = await screen.findByRole("region", { name: "Steering coach" });
  await within(panel).findByText("Score: 2 / 10");
  expect(submits()).toHaveLength(0);

  fireEvent.change(draftInput(), { target: { value: "Fix it. cargo test now" } });
  expect(panel.getAttribute("data-stale")).toBe("true");
  expect(submits()).toHaveLength(0);

  fireEvent.click(screen.getByRole("button", { name: "Re-check draft" }));
  await waitFor(() => expect(panel.getAttribute("data-digest")).toBe("digest:Fix it. cargo test now"));
  expect(submits()).toHaveLength(0);

  fireEvent.click(screen.getByRole("button", { name: "Send prompt" }));
  await waitFor(() => expect(submits()).toHaveLength(1));
  expect(submits()[0][1]).toEqual({ key: "feature:Agent", text: "Fix it. cargo test now" });
  await waitFor(() => expect(screen.queryByRole("region", { name: "Steering coach" })).toBeNull());
  client.clear();
});
