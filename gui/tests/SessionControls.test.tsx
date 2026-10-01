// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { Feature, FeatureSession } from "../src/api";
import {
  SessionStartStopButton,
  sessionRunning,
  stoppedSessionCount,
} from "../src/SessionControls";

afterEach(cleanup);

function session(id: string, extra: Partial<FeatureSession> = {}): FeatureSession {
  return { id, kind: "claude", label: id, tmux_window: id, ...extra };
}

function feature(status: Feature["status"], sessions: FeatureSession[]): Feature {
  return {
    id: "feat-1",
    name: "feat",
    branch: "feat",
    workdir: "/tmp/feat",
    is_worktree: true,
    status,
    agent: "claude",
    mode: "vibe",
    sessions,
  };
}

it("treats a session as running only while its feature runs and it wasn't stopped", () => {
  const live = session("claude");
  const stopped = session("codex", { kind: "codex", stopped: true });
  const todos = session("todos", { kind: "todos" });

  const running = feature("idle", [live, stopped, todos]);
  expect(sessionRunning(running, live)).toBe(true);
  expect(sessionRunning(running, stopped)).toBe(false);
  expect(sessionRunning(running, todos)).toBe(false);
  expect(sessionRunning(feature("stopped", [live]), live)).toBe(false);
  expect(stoppedSessionCount(running)).toBe(1);
});

it("offers stop for a running session and start for a stopped one", () => {
  const onStart = vi.fn();
  const onStop = vi.fn();
  const { rerender } = render(
    <SessionStartStopButton label="Codex 1" running starting={false} stopping={false}
      onStart={onStart} onStop={onStop} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Stop session Codex 1" }));
  expect(onStop).toHaveBeenCalledOnce();

  rerender(
    <SessionStartStopButton label="Codex 1" running={false} starting={false} stopping={false}
      onStart={onStart} onStop={onStop} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Start session Codex 1" }));
  expect(onStart).toHaveBeenCalledOnce();
});
