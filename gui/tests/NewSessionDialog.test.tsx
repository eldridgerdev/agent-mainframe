// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import NewSessionDialog from "../src/NewSessionDialog";
import type { NewSessionOptions } from "../src/sessionsApi";

afterEach(cleanup);

const options: NewSessionOptions = {
  builtin: [
    { kind: "claude", label: "Claude", disabled: null },
    { kind: "terminal", label: "Terminal", disabled: null },
  ],
  custom: [],
  config_warning: null,
  feature_stopped: false,
};

it("creates the selected session with a trimmed custom name", () => {
  const onCreate = vi.fn();
  render(<NewSessionDialog options={options} preferredKind="claude" busy={false} preCheckFailure={null}
    onCreate={onCreate} onClose={vi.fn()} />);

  fireEvent.click(screen.getByRole("radio", { name: "Terminal" }));
  fireEvent.change(screen.getByRole("textbox", { name: /Session name/ }), {
    target: { value: "  Build shell  " },
  });
  fireEvent.click(screen.getByRole("button", { name: "Create session" }));
  expect(onCreate).toHaveBeenCalledWith({ type: "builtin", kind: "terminal" }, "Build shell");
});

it("uses the default name when the optional name is blank", () => {
  const onCreate = vi.fn();
  render(<NewSessionDialog options={options} preferredKind="claude" busy={false} preCheckFailure={null}
    onCreate={onCreate} onClose={vi.fn()} />);

  fireEvent.click(screen.getByRole("button", { name: "Create session" }));
  expect(onCreate).toHaveBeenCalledWith({ type: "builtin", kind: "claude" }, null);
});
