// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import {
  Menu,
  MenuEntry,
  Modal,
  Segmented,
  Switch,
  TOAST_ERROR_MS,
  TOAST_INFO_MS,
  Toast,
  Toasts,
  addToast,
  menuHeightEstimate,
  onTablistKeyDown,
} from "../src/ui";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const esc = () => fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
const tab = (shiftKey = false) => fireEvent.keyDown(document.activeElement ?? document.body, { key: "Tab", shiftKey });

/** A page with a trigger that opens a dialog, which can open a confirm. */
function Stack({ onOuterClose = () => {}, confirmDismissable = true }: { onOuterClose?: () => void; confirmDismissable?: boolean }) {
  const [outer, setOuter] = useState(false);
  const [confirm, setConfirm] = useState(false);
  return (
    <div>
      <main>
        <button onClick={() => setOuter(true)}>Open settings</button>
      </main>
      {outer && (
        <Modal label="Settings" title="Settings" onClose={() => { onOuterClose(); setOuter(false); }}
          footer={<button onClick={() => setConfirm(true)}>Delete…</button>}>
          <label>Name <input aria-label="Name" /></label>
          {confirm && (
            <Modal label="Confirm delete" title="Delete this?" size="sm" dismissable={confirmDismissable}
              onClose={() => setConfirm(false)}
              footer={<><button onClick={() => setConfirm(false)}>Cancel</button><button>Delete</button></>}>
              <p>This cannot be undone.</p>
            </Modal>
          )}
        </Modal>
      )}
      <Toasts toasts={[]} onDismiss={() => {}} />
    </div>
  );
}

it("closes only the dialog in front on Escape, then the one beneath", () => {
  const onOuterClose = vi.fn();
  render(<Stack onOuterClose={onOuterClose} />);
  fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
  expect(screen.getByRole("dialog", { name: "Confirm delete" })).toBeTruthy();

  esc();
  expect(screen.queryByRole("dialog", { name: "Confirm delete" })).toBeNull();
  expect(screen.getByRole("dialog", { name: "Settings" })).toBeTruthy();
  expect(onOuterClose).not.toHaveBeenCalled();

  esc();
  expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
  expect(onOuterClose).toHaveBeenCalledTimes(1);
});

it("lets a non-dismissable dialog swallow Escape instead of closing the one beneath", () => {
  const onOuterClose = vi.fn();
  render(<Stack onOuterClose={onOuterClose} confirmDismissable={false} />);
  fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
  esc();
  expect(screen.getByRole("dialog", { name: "Confirm delete" })).toBeTruthy();
  expect(onOuterClose).not.toHaveBeenCalled();
});

it("moves focus in, traps Tab and returns focus to the trigger", () => {
  render(<Stack />);
  const trigger = screen.getByRole("button", { name: "Open settings" });
  trigger.focus();
  fireEvent.click(trigger);
  const dialog = screen.getByRole("dialog", { name: "Settings" });
  // The first text field takes focus.
  expect(document.activeElement).toBe(screen.getByLabelText("Name"));

  const close = screen.getByRole("button", { name: "Close" });
  const last = screen.getByRole("button", { name: "Delete…" });
  last.focus();
  tab();
  expect(document.activeElement).toBe(close);
  tab(true);
  expect(document.activeElement).toBe(last);
  expect(dialog.contains(document.activeElement)).toBe(true);

  esc();
  expect(document.activeElement).toBe(trigger);
});

it("returns focus to the dialog control that opened a confirm", () => {
  render(<Stack />);
  fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
  const opener = screen.getByRole("button", { name: "Delete…" });
  opener.focus();
  fireEvent.click(opener);
  const confirm = screen.getByRole("dialog", { name: "Confirm delete" });
  // No text field: the dialog itself takes focus, and Tab stays inside it.
  expect(document.activeElement).toBe(confirm);
  tab();
  expect(confirm.contains(document.activeElement)).toBe(true);
  esc();
  expect(document.activeElement).toBe(opener);
});

it("keeps an autoFocus field focused on open", () => {
  render(
    <Modal label="Rename" title="Rename" onClose={() => {}}>
      <input aria-label="First" />
      <input aria-label="Second" autoFocus />
    </Modal>,
  );
  expect(document.activeElement).toBe(screen.getByLabelText("Second"));
});

it("makes the page behind the front dialog inert, except the toasts, and restores it", () => {
  const { container } = render(<Stack />);
  fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
  const main = container.querySelector("main")!;
  const toasts = container.querySelector(".toasts")!;
  expect(main.hasAttribute("inert")).toBe(true);
  expect(toasts.hasAttribute("inert")).toBe(false);

  fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
  // The settings form behind the confirm is inert too.
  expect(screen.getByLabelText("Name").closest("[inert]")).not.toBeNull();
  esc();
  expect(screen.getByLabelText("Name").closest("[inert]")).toBeNull();
  esc();
  expect(main.hasAttribute("inert")).toBe(false);
});

it("does not register a hidden dialog, and resumes its focus when shown again", () => {
  function Hiding() {
    const [front, setFront] = useState(false);
    return <>
      <Modal label="Library" title="Library" hidden={front} onClose={() => {}}
        footer={<button onClick={() => setFront(true)}>Edit</button>}>
        <p>Templates</p>
      </Modal>
      {front && <Modal label="Editor" title="Editor" onClose={() => setFront(false)}><p>Editing</p></Modal>}
    </>;
  }
  render(<Hiding />);
  const edit = screen.getByRole("button", { name: "Edit" });
  edit.focus();
  fireEvent.click(edit);
  esc();
  expect(screen.queryByRole("dialog", { name: "Editor" })).toBeNull();
  expect(screen.getByRole("dialog", { name: "Library" })).toBeTruthy();
  expect(document.activeElement).toBe(edit);
});

it("names the dialog by its heading when the label repeats the title", () => {
  render(
    <>
      <Modal label="Settings" title="Settings" onClose={() => {}}><p>a</p></Modal>
    </>,
  );
  const dialog = screen.getByRole("dialog", { name: "Settings" });
  expect(dialog.getAttribute("aria-label")).toBeNull();
  expect(document.getElementById(dialog.getAttribute("aria-labelledby")!)?.tagName).toBe("H2");
  cleanup();
  render(<Modal label="Diff viewer" title="Changes · feat" onClose={() => {}}><p>a</p></Modal>);
  expect(screen.getByRole("dialog", { name: "Diff viewer" }).getAttribute("aria-label")).toBe("Diff viewer");
});

// ── Menu ───────────────────────────────────────────────────

function menuEntries(onSelect = vi.fn()): MenuEntry[] {
  return [
    { section: "Plan" },
    { label: "Plan interview", onSelect, shortcut: "P" },
    { label: "Quick Plan", onSelect, disabled: true, hint: "Working…" },
    "separator",
    { section: "Review" },
    { label: "Final Review", onSelect },
    { label: "Learning", onSelect, title: "Ask about the code" },
  ];
}

it("renders sections, separators, shortcuts and a trigger tooltip", () => {
  render(<Menu label="Tools" text="Tools" icon={null} title="Plan, review and PR tools" items={menuEntries()} indicator />);
  const trigger = screen.getByRole("button", { name: "Tools" });
  expect(trigger.getAttribute("title")).toBe("Plan, review and PR tools");
  expect(trigger.querySelector(".menu-trigger-dot")).not.toBeNull();
  fireEvent.click(trigger);
  const menu = screen.getByRole("menu", { name: "Tools" });
  expect(trigger.getAttribute("aria-controls")).toBe(menu.id);
  expect(screen.getByRole("group", { name: "Plan" })).toBeTruthy();
  expect(screen.getByRole("group", { name: "Review" })).toBeTruthy();
  expect(screen.getAllByRole("separator")).toHaveLength(1);
  expect(screen.getByRole("menuitem", { name: /Plan interview/ }).querySelector("kbd")?.textContent).toBe("P");
  expect(screen.getByRole("menuitem", { name: /Learning/ }).getAttribute("title")).toBe("Ask about the code");
});

it("keeps plain item lists working", () => {
  const onSelect = vi.fn();
  render(<Menu label="More" items={[{ label: "Rename", onSelect }, { label: "Delete", onSelect, danger: true }]} />);
  fireEvent.click(screen.getByRole("button", { name: "More" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
  expect(onSelect).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("menu")).toBeNull();
});

it("supports the full keyboard model and returns focus to the trigger", () => {
  const onSelect = vi.fn();
  render(<Menu label="Tools" text="Tools" items={menuEntries(onSelect)} />);
  const trigger = screen.getByRole("button", { name: "Tools" });
  trigger.focus();
  fireEvent.click(trigger);
  const item = (name: RegExp) => screen.getByRole("menuitem", { name });
  // Focus lands on the first enabled item; disabled items are skipped.
  expect(document.activeElement).toBe(item(/Plan interview/));
  fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
  expect(document.activeElement).toBe(item(/Final Review/));
  fireEvent.keyDown(document.activeElement!, { key: "End" });
  expect(document.activeElement).toBe(item(/Learning/));
  fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
  expect(document.activeElement).toBe(item(/Plan interview/));
  fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
  expect(document.activeElement).toBe(item(/Learning/));
  fireEvent.keyDown(document.activeElement!, { key: "Home" });
  expect(document.activeElement).toBe(item(/Plan interview/));

  esc();
  expect(screen.queryByRole("menu")).toBeNull();
  expect(document.activeElement).toBe(trigger);

  // ArrowUp on the trigger opens on the last item; Tab closes.
  fireEvent.keyDown(trigger, { key: "ArrowUp" });
  expect(document.activeElement).toBe(item(/Learning/));
  tab();
  expect(screen.queryByRole("menu")).toBeNull();
  expect(document.activeElement).toBe(trigger);

  // ArrowDown opens on the first; choosing returns focus before the action runs.
  fireEvent.keyDown(trigger, { key: "ArrowDown" });
  onSelect.mockImplementation(() => expect(document.activeElement).toBe(trigger));
  fireEvent.click(item(/Final Review/));
  expect(onSelect).toHaveBeenCalledTimes(1);
});

it("closes a menu inside a dialog without closing the dialog", () => {
  const onClose = vi.fn();
  render(
    <Modal label="Settings" title="Settings" onClose={onClose}>
      <Menu label="Row actions" items={[{ label: "Rename", onSelect: () => {} }]} />
    </Modal>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Row actions" }));
  expect(screen.getByRole("menu")).toBeTruthy();
  esc();
  expect(screen.queryByRole("menu")).toBeNull();
  expect(onClose).not.toHaveBeenCalled();
  esc();
  expect(onClose).toHaveBeenCalledTimes(1);
});

it("estimates menu height from items, sections and separators", () => {
  expect(menuHeightEstimate([])).toBe(12);
  expect(menuHeightEstimate(menuEntries())).toBe(12 + 4 * 34 + 2 * 24 + 9);
});

// ── Segmented, Switch, tabs ────────────────────────────────

it("gives Segmented one Tab stop and arrow-key selection", () => {
  function Harness() {
    const [value, setValue] = useState<"a" | "b" | "c">("b");
    return <Segmented label="Mode" value={value} onChange={setValue}
      options={[{ value: "a", label: "Alpha" }, { value: "b", label: "Beta" }, { value: "c", label: "Gamma" }]} />;
  }
  render(<Harness />);
  const radio = (name: string) => screen.getByRole("radio", { name });
  expect(radio("Beta").tabIndex).toBe(0);
  expect(radio("Alpha").tabIndex).toBe(-1);
  radio("Beta").focus();
  fireEvent.keyDown(radio("Beta"), { key: "ArrowRight" });
  expect(radio("Gamma").getAttribute("aria-checked")).toBe("true");
  expect(document.activeElement).toBe(radio("Gamma"));
  fireEvent.keyDown(radio("Gamma"), { key: "ArrowRight" });
  expect(radio("Alpha").getAttribute("aria-checked")).toBe("true");
  fireEvent.keyDown(radio("Alpha"), { key: "End" });
  expect(radio("Gamma").getAttribute("aria-checked")).toBe("true");
  expect(radio("Gamma").tabIndex).toBe(0);
});

it("exposes Switch as a switch", () => {
  const onChange = vi.fn();
  render(<Switch checked={false} onChange={onChange} label="Chrome" />);
  fireEvent.click(screen.getByRole("switch", { name: "Chrome" }));
  expect(onChange).toHaveBeenCalledWith(true);
});

it("moves focus between tabs with the arrow keys", () => {
  render(
    <div role="tablist" onKeyDown={onTablistKeyDown}>
      <button role="tab" tabIndex={0}>One</button>
      <button role="tab" tabIndex={-1}>Two</button>
      <button role="tab" tabIndex={-1}>Three</button>
    </div>,
  );
  const one = screen.getByRole("tab", { name: "One" });
  one.focus();
  fireEvent.keyDown(one, { key: "ArrowRight" });
  expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Two" }));
  fireEvent.keyDown(document.activeElement!, { key: "ArrowLeft" });
  fireEvent.keyDown(document.activeElement!, { key: "ArrowLeft" });
  expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Three" }));
  fireEvent.keyDown(document.activeElement!, { key: "Home" });
  expect(document.activeElement).toBe(one);
});

// ── Toasts ─────────────────────────────────────────────────

const toast = (id: number, tone: Toast["tone"], key?: string): Toast =>
  ({ id, tone, title: `t${id}`, message: `m${id}`, key });

it("drops info toasts before errors and replaces a toast with the same key", () => {
  const errors = [toast(1, "error"), toast(2, "error"), toast(3, "info"), toast(4, "error")];
  // A new error evicts the info toast, never an unread error.
  expect(addToast(errors, toast(5, "error")).map((t) => t.id)).toEqual([1, 2, 4, 5]);
  // With only errors on screen, an incoming info toast is the one dropped.
  expect(addToast([1, 2, 4, 5].map((id) => toast(id, "error")), toast(6, "info")).map((t) => t.id)).toEqual([1, 2, 4, 5]);
  // Only when everything is an error does the oldest error go.
  expect(addToast([1, 2, 3, 4].map((id) => toast(id, "error")), toast(5, "error")).map((t) => t.id)).toEqual([2, 3, 4, 5]);
  // Same action key: the retry's toast replaces the old one.
  expect(addToast([toast(1, "error", "add"), toast(2, "info")], toast(3, "error", "add")).map((t) => t.id)).toEqual([2, 3]);
});

it("keeps error toasts longer, pauses on hover and focus, and announces politely", () => {
  vi.useFakeTimers();
  const onDismiss = vi.fn();
  const { container } = render(<Toasts toasts={[toast(1, "info"), toast(2, "error")]} onDismiss={onDismiss} />);
  expect(container.querySelector(".toasts")?.getAttribute("aria-live")).toBe("polite");
  const info = screen.getByRole("status");
  const error = screen.getByRole("alert");

  // Hovering the info toast stops its clock.
  act(() => { vi.advanceTimersByTime(TOAST_INFO_MS - 1000); });
  fireEvent.mouseEnter(info);
  act(() => { vi.advanceTimersByTime(10_000); });
  expect(onDismiss).not.toHaveBeenCalledWith(1);
  fireEvent.mouseLeave(info);
  act(() => { vi.advanceTimersByTime(1000); });
  expect(onDismiss).toHaveBeenCalledWith(1);

  // Focus inside the error toast also pauses it.
  fireEvent.focus(screen.getAllByRole("button", { name: "Dismiss" })[1]);
  act(() => { vi.advanceTimersByTime(TOAST_ERROR_MS); });
  expect(onDismiss).not.toHaveBeenCalledWith(2);
  fireEvent.blur(screen.getAllByRole("button", { name: "Dismiss" })[1]);
  act(() => { vi.advanceTimersByTime(TOAST_ERROR_MS - TOAST_INFO_MS - 9_000); });
  expect(onDismiss).toHaveBeenCalledWith(2);
  expect(error).toBeTruthy();
});

it("releases the page when an open dialog's wrapper is hidden (a minimized dialog)", async () => {
  const onClose = vi.fn();
  function Minimizable() {
    const [minimized, setMinimized] = useState(false);
    return (
      <div>
        <main><button onClick={() => setMinimized(false)}>Restore</button></main>
        <div hidden={minimized}>
          <Modal label="Plan" title="Plan" dismissable={false} onClose={onClose}
            footer={<button onClick={() => setMinimized(true)}>Minimize</button>}>
            <p>Interview</p>
          </Modal>
        </div>
      </div>
    );
  }
  render(<Minimizable />);
  const main = document.querySelector("main")!;
  expect(main.hasAttribute("inert")).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Minimize" }));
  await act(async () => { await Promise.resolve(); });
  expect(main.hasAttribute("inert")).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "Restore" }));
  await act(async () => { await Promise.resolve(); });
  expect(main.hasAttribute("inert")).toBe(true);
});
