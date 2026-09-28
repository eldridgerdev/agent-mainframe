// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import PlanPanel from "../src/PlanPanel";
import type { PlanView } from "../src/api";

afterEach(cleanup);

const brief: PlanView = {
  interview_key: "feature-1",
  feature_name: "Feature one",
  kind: "full",
  phase: "brief",
  step_key: "feature-1:brief:0:0:docs0",
  question_index: 0,
  question_count: 0,
  question: null,
  editor_text: "Original brief",
  selected_option: null,
  review_markdown: null,
  critique: null,
  attached_docs: [],
  kickoff_target: null,
};

describe("PlanPanel", () => {
  it("keeps an unsaved answer when a poll returns the same plan step", () => {
    const onAct = vi.fn(async () => {});
    const { rerender } = render(
      <PlanPanel view={brief} precall={null} busy={false} onAct={onAct} />,
    );
    const answer = screen.getByRole("textbox", { name: "Plan answer" }) as HTMLTextAreaElement;
    fireEvent.change(answer, { target: { value: "My unsaved answer" } });

    rerender(
      <PlanPanel view={{ ...brief }} precall={null} busy={false} onAct={onAct} />,
    );
    expect(answer.value).toBe("My unsaved answer");
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(onAct).toHaveBeenCalledWith("next", {
      text: "My unsaved answer",
      selected_option: null,
    });
  });

  it("holds the interview behind the headless-call notice", () => {
    const onAct = vi.fn(async () => {});
    const precall = {
      title: "Plan synthesis",
      harness: "Claude",
      preview: "Rendered planning prompt",
      viewing: false,
    };
    const { rerender } = render(
      <PlanPanel view={brief} precall={precall} busy={false} onAct={onAct} />,
    );
    expect(screen.getByRole("dialog", { name: "Approve planning agent call" })).toBeTruthy();
    expect(screen.queryByRole("textbox", { name: "Plan answer" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "View prompt" }));
    expect(onAct).toHaveBeenCalledWith("precall_toggle_view");

    rerender(
      <PlanPanel view={{ ...brief }} precall={{ ...precall, viewing: true }} busy={false} onAct={onAct} />,
    );
    expect(screen.getByText("Rendered planning prompt")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(onAct).toHaveBeenCalledWith("precall_confirm", undefined);
  });
});
