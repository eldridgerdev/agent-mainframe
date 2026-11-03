// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import PromptComposer from "../src/PromptComposer";

afterEach(cleanup);

function mount(text: string, onSaveTemplate = vi.fn()) {
  render(<PromptComposer text={text} sending={false} ready focusRequested={false} onFocusHandled={vi.fn()}
    onChange={vi.fn()} onClear={vi.fn()} onSend={vi.fn()} onLibrary={vi.fn()} onSaveTemplate={onSaveTemplate}
    onSteeringChange={vi.fn()} />);
  return onSaveTemplate;
}

it("keeps one toolbar row and moves Save as template into its overflow menu", () => {
  const onSaveTemplate = mount("Fix the cart total");
  const head = document.querySelector(".composer-head") as HTMLElement;
  expect(Array.from(head.querySelectorAll("button")).map((b) => b.getAttribute("aria-label") ?? b.textContent?.trim()))
    .toEqual(["Steering coach", "Prompt library", "Clear", "More draft actions"]);
  fireEvent.click(screen.getByRole("button", { name: "More draft actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Save as template" }));
  expect(onSaveTemplate).toHaveBeenCalledTimes(1);
});

it("says why Save as template is unavailable for an empty draft", () => {
  mount("  ");
  fireEvent.click(screen.getByRole("button", { name: "More draft actions" }));
  const item = screen.getByRole("menuitem", { name: /Save as template/ }) as HTMLButtonElement;
  expect(item.disabled).toBe(true);
  expect(item.textContent).toContain("Write a draft first");
});
