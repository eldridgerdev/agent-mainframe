// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import ProjectsSidebar from "../src/ProjectsSidebar";
import { resetSidebarPrefsForTest, useSidebarCollapsed, useSidebarShortcut } from "../src/sidebarPrefs";

function AgentPanel() {
  const [collapsed, set] = useSidebarCollapsed("sessionSidebar");
  useSidebarShortcut("sessionSidebar", () => set(!collapsed));
  return <button onClick={() => set(!collapsed)}>{collapsed ? "Show agent" : "Hide agent"}</button>;
}
function Layout() {
  return <><ProjectsSidebar><nav aria-label="Workspace">Projects</nav></ProjectsSidebar><AgentPanel /></>;
}
beforeEach(() => { localStorage.clear(); resetSidebarPrefsForTest(); });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
const chord = (target: Element | Window, key: string) =>
  fireEvent.keyDown(target, { key, altKey: true, shiftKey: true });

it("keeps independent choices across remounts and fresh preference reads", () => {
  const first = render(<Layout />);
  fireEvent.click(screen.getByRole("button", { name: "Hide projects sidebar" }));
  expect(screen.queryByRole("navigation")).toBeNull();
  expect(screen.getByRole("button", { name: "Hide agent" })).toBeTruthy();
  expect(localStorage.getItem("amf.gui.collapsed.projectsSidebar")).toBe("1");
  first.unmount();
  resetSidebarPrefsForTest();
  render(<Layout />);
  expect(screen.getByRole("button", { name: "Show projects sidebar" }).getAttribute("aria-expanded")).toBe("false");
  chord(window, "A");
  chord(window, "P");
  expect(screen.getByRole("navigation", { name: "Workspace" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Show agent" })).toBeTruthy();
  expect(localStorage.getItem("amf.gui.collapsed.projectsSidebar")).toBe("0");
  expect(localStorage.getItem("amf.gui.collapsed.sessionSidebar")).toBe("1");
});

it("leaves terminal, editable, repeated and dialog keys alone", () => {
  render(<><Layout /><div className="term-frame"><button>Terminal</button></div>
    <input aria-label="Input" /><textarea aria-label="Draft" /><div contentEditable>Editable</div></>);
  for (const target of [screen.getByText("Terminal"), screen.getByLabelText("Input"), screen.getByLabelText("Draft"), screen.getByText("Editable")]) {
    chord(target, "P"); chord(target, "A");
  }
  fireEvent.keyDown(window, { key: "P", altKey: true, shiftKey: true, repeat: true });
  fireEvent.keyDown(window, { key: "P", altKey: true });
  const dialog = document.createElement("div");
  dialog.setAttribute("role", "dialog"); document.body.appendChild(dialog);
  chord(window, "P"); chord(window, "A"); dialog.remove();
  expect(screen.getByRole("button", { name: "Hide projects sidebar" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Hide agent" })).toBeTruthy();
  // Option can change event.key on macOS; the physical chord still works.
  fireEvent.keyDown(window, { key: "∏", code: "KeyP", altKey: true, shiftKey: true });
  expect(screen.getByRole("button", { name: "Show projects sidebar" })).toBeTruthy();
});

it("keeps a chosen state through narrow-window resizes and storage failure", () => {
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
  const view = render(<Layout />);
  fireEvent.click(screen.getByRole("button", { name: "Hide projects sidebar" }));
  fireEvent.resize(window, { target: { innerWidth: 480 } });
  expect(screen.getByRole("button", { name: "Show projects sidebar" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Show projects sidebar" }));
  view.unmount();
  render(<Layout />);
  expect(screen.getByRole("button", { name: "Hide projects sidebar" })).toBeTruthy();
});


it.each(["dialog", "alertdialog"])("blocks only visible %s instances, including a minimized planning wrapper", (role) => {
  const frame = (hidden: boolean, otherDialog = false) => <>
    <Layout />
    <div hidden={hidden}><div role={role}>Plan interview</div></div>
    {otherDialog && <div role="dialog">Visible confirmation</div>}
  </>;
  const view = render(frame(true));
  // Minimized planning stays mounted but must leave workspace shortcuts available.
  chord(window, "P"); chord(window, "A");
  expect(screen.getByRole("button", { name: "Show projects sidebar" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Show agent" })).toBeTruthy();

  view.rerender(frame(false));
  chord(window, "P"); chord(window, "A");
  expect(screen.getByRole("button", { name: "Show projects sidebar" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Show agent" })).toBeTruthy();

  // A second visible dialog still blocks shortcuts when the first is hidden.
  view.rerender(frame(true, true));
  chord(window, "P"); chord(window, "A");
  expect(screen.getByRole("button", { name: "Show projects sidebar" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Show agent" })).toBeTruthy();

  view.rerender(frame(true));
  chord(window, "P"); chord(window, "A");
  expect(screen.getByRole("button", { name: "Hide projects sidebar" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Hide agent" })).toBeTruthy();
});

it.each(["display: none", "visibility: hidden"])("ignores dialogs concealed by an ancestor with %s", (style) => {
  render(<Layout />);
  const wrapper = document.createElement("div");
  wrapper.setAttribute("style", style);
  wrapper.innerHTML = '<div role="dialog">Hidden dialog</div>';
  document.body.appendChild(wrapper);
  try {
    chord(window, "P"); chord(window, "A");
    expect(screen.getByRole("button", { name: "Show projects sidebar" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Show agent" })).toBeTruthy();
  } finally { wrapper.remove(); }
});
