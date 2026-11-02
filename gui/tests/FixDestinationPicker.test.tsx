// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import FixDestinationPicker, { preselectedTarget, resolveTarget, targetKey } from "../src/FixDestinationPicker";
import type { PrFixTarget } from "../src/prTriageApi";
afterEach(cleanup);

const row = (feature: string, session: string, kind: PrFixTarget["destination_kind"], stopped = false): PrFixTarget => ({
  target: { project_id: "p", feature_id: feature, session_id: session }, label: session, harness: "claude", stopped,
  destination_revision: `${feature}-${session}`, destination_kind: kind, feature_name: feature,
});
const opened = row("main", "agent", "opened_feature");
const companion = row("fix-triage", "triage", "companion");

it("preselects only a running companion session, never the first row", () => {
  expect(preselectedTarget([opened])).toBeNull();
  expect(preselectedTarget([opened, row("fix-triage", "stopped", "companion", true)])).toBeNull();
  expect(preselectedTarget([opened, companion])).toBe(companion);
});

it("resolves explicit choices against the live catalog and drops vanished ones", () => {
  expect(resolveTarget([opened, companion], "")).toBeNull();
  expect(resolveTarget([opened, companion], targetKey(opened))).toBe(opened);
  expect(resolveTarget([opened], targetKey(companion))).toBeNull();
});

it("groups sessions by feature kind, disables stopped rows and reports the chosen key", () => {
  const onChoose = vi.fn();
  render(<FixDestinationPicker label="Fix agent" targets={[opened, row("main", "idle", "opened_feature", true), companion]} choice={null} onChoose={onChoose} />);
  const select = screen.getByRole("combobox", { name: "Fix agent" }) as HTMLSelectElement;
  expect(select.value).toBe(targetKey(companion));
  expect((screen.getByRole("option", { name: "idle (stopped)" }) as HTMLOptionElement).disabled).toBe(true);
  fireEvent.change(select, { target: { value: targetKey(opened) } });
  expect(onChoose).toHaveBeenCalledWith(targetKey(opened));
});

it("hides stopped rows in hide mode and flags a stale choice", () => {
  render(<FixDestinationPicker label="Fix agent" targets={[opened, row("main", "idle", "opened_feature", true)]} choice={"gone"} stoppedMode="hide" onChoose={vi.fn()} />);
  expect(screen.queryByRole("option", { name: /idle/ })).toBeNull();
  expect(screen.getByRole("option", { name: /unavailable or changed/ })).toBeTruthy();
});
