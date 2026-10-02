import { expect, it } from "vitest";
import { Feature, FeatureSession } from "../src/api";
import { closingStopsFeature } from "../src/SessionControls";

const session = (id: string, kind = "terminal"): FeatureSession => ({ id, kind, label: id, tmux_window: id });

const feature = (sessions: FeatureSession[], status: Feature["status"] = "idle"): Feature => ({
  id: "feat", name: "feat", branch: "feat", workdir: "/w", is_worktree: true,
  status, agent: "claude", mode: "vibeless", sessions,
});

it("warns when the closed tab is the last live window, even beside stopped and Todos tabs", () => {
  const f = feature([session("live"), session("stopped", "codex"), session("list", "todos")]);

  expect(closingStopsFeature(f, "live", ["stopped"])).toBe(true);
});

it("does not warn while another tab still has a window", () => {
  const f = feature([session("a"), session("b"), session("list", "todos")]);

  expect(closingStopsFeature(f, "a", [])).toBe(false);
});

it("does not warn when closing a stopped tab or a Todos tab beside a live one", () => {
  const f = feature([session("live"), session("stopped"), session("list", "todos")]);

  expect(closingStopsFeature(f, "stopped", ["stopped"])).toBe(false);
  expect(closingStopsFeature(f, "list", ["stopped"])).toBe(false);
});

it("warns for a running feature's only session, whatever its kind", () => {
  expect(closingStopsFeature(feature([session("list", "todos")]), "list", [])).toBe(true);
});

it("never warns for a feature that is already stopped", () => {
  expect(closingStopsFeature(feature([session("only")], "stopped"), "only", [])).toBe(false);
});

it("counts a tab stopped on its own as not running, even before the snapshot lists it", () => {
  const f = feature([session("live"), { ...session("flagged"), stopped: true }]);

  expect(closingStopsFeature(f, "live", [])).toBe(true);
});
