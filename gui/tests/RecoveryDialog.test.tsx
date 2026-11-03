// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import RecoveryDialog from "../src/RecoveryDialog";

afterEach(cleanup);

const option = { harness: "Claude", saved_id: "saved-claude-id" };

it("offers resume, fresh start, and the saved-session picker after tmux exits", () => {
  const onRecover = vi.fn();
  const onChoose = vi.fn();
  render(
    <RecoveryDialog option={option} sessions={null} selectedId={null} loading={false} busy={false}
      onChoose={onChoose} onBack={vi.fn()} onSelect={vi.fn()} onRecover={onRecover} onClose={vi.fn()} />,
  );

  // The saved id is an internal detail; the choices are described in words.
  expect(screen.queryByText(/saved-claude-id/)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Continue where it left off" }));
  expect(onRecover).toHaveBeenCalledWith("resume", null);
  fireEvent.click(screen.getByRole("button", { name: "Start a new conversation" }));
  expect(onRecover).toHaveBeenCalledWith("clear", null);
  fireEvent.click(screen.getByRole("button", { name: "Pick an older conversation" }));
  expect(onChoose).toHaveBeenCalledOnce();
});

it("passes the chosen saved transcript to recovery", () => {
  const onRecover = vi.fn();
  function Harness() {
    const [selectedId, setSelectedId] = useState("older-id");
    return <RecoveryDialog option={option} sessions={[
      { id: "older-id", title: "Earlier work", updated: 1 },
      { id: "newer-id", title: "Current work", updated: 2 },
    ]} selectedId={selectedId} loading={false} busy={false}
      onChoose={vi.fn()} onBack={vi.fn()} onSelect={setSelectedId} onRecover={onRecover} onClose={vi.fn()} />;
  }
  render(<Harness />);

  fireEvent.click(screen.getByText("Current work"));
  fireEvent.click(screen.getByRole("button", { name: "Continue this conversation" }));
  expect(onRecover).toHaveBeenCalledWith("pick", "newer-id");
});
