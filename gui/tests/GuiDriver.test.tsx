// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, expect, it } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import { useState } from "react";

const script = readFileSync(`${process.cwd()}/../scripts/dev/gui-driver/dom.js`, "utf8");
type Action = { op: string; [key: string]: unknown };
type Snapshot = { text: string; text_truncated: boolean; controls_truncated: boolean; controls: Array<{ ref: string; name: string; value?: string; disabled?: boolean }> };
const run = (action: Action): Snapshot => (window as unknown as { __amfGuiDriver: (action: Action) => Snapshot }).__amfGuiDriver(action);

beforeEach(() => {
  document.body.innerHTML = "";
  new Function("window", "document", script)(window, document);
});
afterEach(cleanup);

it("omits hidden and terminal content while retaining labelled, bounded controls", () => {
  document.body.innerHTML = `<p>Workspace ready</p><div hidden><button>Hidden</button></div>
    <div style="display:none"><button>Also hidden</button></div>
    <div class="xterm">Huge terminal output<button>Terminal</button></div>
    <label for="name">Project name</label><input id="name" value="demo">
    <button aria-label="New project">+</button><button disabled>Delete</button>`;
  const snapshot = run({ op: "snapshot" });
  expect(snapshot.text).toContain("Workspace ready");
  expect(snapshot.text).not.toMatch(/Hidden|Also hidden|Huge terminal|Terminal/);
  expect(snapshot.controls.map(control => control.name)).toEqual(["Project name", "New project", "Delete"]);
  expect(snapshot.controls[0].value).toBe("demo");
  expect(snapshot.controls[2].disabled).toBe(true);
  const small = run({ op: "snapshot", text_limit: 4, control_limit: 1 });
  expect(small.text).toHaveLength(4);
  expect(small.text_truncated).toBe(true);
  expect(small.controls_truncated).toBe(true);
});

it("rejects ambiguous controls and disabled actions and permits scoped exact selection", () => {
  document.body.innerHTML = '<div id="a"><button>Save</button></div><div id="b"><button>Save</button></div><button disabled>Delete</button>';
  expect(() => run({ op: "click", name: "Save" })).toThrow("2 visible elements");
  expect(() => run({ op: "click", name: "Sav" })).toThrow("0 visible elements");
  expect(() => run({ op: "click", name: "Delete" })).toThrow("disabled");
  let clicks = 0;
  document.querySelector("#b button")!.addEventListener("click", () => clicks++);
  run({ op: "click", name: "Save", scope: "#b" });
  expect(clicks).toBe(1);
});

it("keeps references stable across snapshots and refuses detached or hidden targets", () => {
  document.body.innerHTML = '<button>First</button>';
  const original = run({ op: "snapshot" }).controls[0].ref;
  expect(run({ op: "snapshot" }).controls[0].ref).toBe(original);
  document.querySelector("button")!.hidden = true;
  expect(() => run({ op: "click", ref: original })).toThrow("0 visible elements");
  document.body.innerHTML = '<button>Replacement</button>';
  expect(() => run({ op: "click", ref: original })).toThrow("Stale reference");
  expect(run({ op: "snapshot" }).controls[0].ref).not.toBe(original);
});

it("matches waited text across inline elements and treats an unrendered scope as not yet", () => {
  const has = (action: Omit<Action, "op">) => run({ op: "has_text", ...action }) as unknown as boolean;
  document.body.innerHTML = `<p>Created <b>Fixture project</b></p><p>Fix<mark>ture</mark> list</p>
    <div><span>Save</span><span hidden>secret</span></div><div>Cancel</div>`;
  expect(has({ text: "Created Fixture project" })).toBe(true);
  expect(has({ text: "Fixture  list" })).toBe(true);
  expect(has({ text: "Save Cancel" })).toBe(true);
  expect(has({ text: "secret" })).toBe(false);
  expect(has({ text: "Created", scope: "[role=dialog]" })).toBe(false);
  document.body.insertAdjacentHTML("beforeend", '<div role="dialog">Created</div>');
  expect(has({ text: "Created", scope: "[role=dialog]" })).toBe(true);
  expect(has({ text: "Fixture", scope: "[role=dialog]" })).toBe(false);
  expect(() => has({ text: "Created", scope: "p" })).toThrow("Scope matched 2 elements");
});

it("marks references issued after a re-injection so they cannot collide with earlier ones", () => {
  document.body.innerHTML = "<button>First</button>";
  const original = run({ op: "snapshot" }).controls[0].ref;
  const page = window as unknown as { __amfGuiDriverEpoch?: number };
  page.__amfGuiDriverEpoch = 1;
  try {
    new Function("window", "document", script)(window, document);
    const reissued = run({ op: "snapshot" }).controls[0].ref;
    expect(original).toBe("e1");
    expect(reissued).toBe("e1-1");
    expect(() => run({ op: "click", ref: original })).toThrow("Stale reference");
  } finally {
    delete page.__amfGuiDriverEpoch;
  }
});

it("updates React controlled inputs, textareas and selects through application events", () => {
  function Form() {
    const [input, setInput] = useState("");
    const [textarea, setTextarea] = useState("");
    const [choice, setChoice] = useState("a");
    return <>
      <label>Project name<input value={input} onChange={event => setInput(event.target.value)} /></label>
      <label>Draft<textarea value={textarea} onChange={event => setTextarea(event.target.value)} /></label>
      <label>Choice<select value={choice} onChange={event => setChoice(event.target.value)}><option value="a">A</option><option value="b">B</option></select></label>
      <output>{JSON.stringify({ input, textarea, choice })}</output>
    </>;
  }
  render(<Form />);
  act(() => { run({ op: "fill", name: "Project name", value: "世界" }); });
  act(() => { run({ op: "fill", name: "Draft", value: "one\ntwo" }); });
  act(() => { run({ op: "select", name: "Choice", value: "b" }); });
  expect(screen.getByRole("status").textContent).toBe(JSON.stringify({ input: "世界", textarea: "one\ntwo", choice: "b" }));
  expect(() => run({ op: "select", name: "Choice", value: "missing" })).toThrow("enabled matching option");
});
