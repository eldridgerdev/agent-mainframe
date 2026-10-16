import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import { blocks, token, parse, over, contrast, type Rgb } from "./contrast";

const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
const light = blocks(css).light;
const rgb = (name: string) => parse(token(light, name)).rgb;
const surfaces = ["bg", "bg-sidebar", "surface", "surface-2"];
const failures: string[] = [];
function check(label: string, fg: Rgb, bg: Rgb, minimum = 4.5) {
  const ratio = contrast(fg, bg);
  if (ratio < minimum) failures.push(`${label}: ${ratio.toFixed(2)} < ${minimum}`);
}

it("keeps light body text readable on every surface", () => {
  failures.length = 0;
  for (const name of [...surfaces, "surface-3"]) {
    for (const text of ["text", "text-muted"]) check(`${text} on ${name}`, rgb(text), rgb(name));
    if (name !== "surface-3") check(`text-faint on ${name}`, rgb("text-faint"), rgb(name));
  }
  expect(failures).toEqual([]);
});

it("keeps light status colours readable as text and on their soft tints", () => {
  failures.length = 0;
  for (const colour of ["accent", "green", "amber", "red"]) {
    for (const name of ["surface", "bg-sidebar"]) {
      check(`${colour} on ${name}`, rgb(colour), rgb(name));
      check(`${colour} on ${colour}-soft/${name}`, rgb(colour), over(token(light, `${colour}-soft`), rgb(name)));
    }
  }
  expect(failures).toEqual([]);
});

it("keeps text on filled colours readable", () => {
  failures.length = 0;
  check("accent-fg on accent", rgb("accent-fg"), rgb("accent"));
  check("accent-fg on accent-hover", rgb("accent-fg"), rgb("accent-hover"));
  check("danger-fg on red", rgb("danger-fg"), rgb("red"));
  check("warning-fg on amber", rgb("warning-fg"), rgb("amber"));
  check("success-fg on green", rgb("success-fg"), rgb("green"));
  expect(failures).toEqual([]);
});

it("draws control boundaries at 3:1 and keeps disabled controls legible", () => {
  failures.length = 0;
  for (const name of surfaces) check(`border-control on ${name}`, rgb("border-control"), rgb(name), 3);
  check("disabled-fg on disabled-bg", rgb("disabled-fg"), rgb("disabled-bg"), 3);
  check("disabled-fg on surface", rgb("disabled-fg"), rgb("surface"), 3);
  expect(failures).toEqual([]);
});

it("keeps light agent-sidebar accents and active-row text readable", () => {
  failures.length = 0;
  const sidebar = blocks(readFileSync(new URL("../src/sessionSidebar.css", import.meta.url), "utf8")).light;
  for (const role of ["harness-claude", "harness-codex", "harness-opencode", "accent-pr", "accent-summary", "accent-prompt"]) {
    for (const name of surfaces) check(`agent sidebar ${role} on ${name}`, parse(token(sidebar, `sb-${role}`)).rgb, rgb(name));
  }
  // The selected nav item and active feature row are the accent tint over
  // the sidebar; their counts and labels are drawn on it.
  const active = over(token(light, "accent-soft"), rgb("bg-sidebar"));
  for (const text of ["text", "text-muted"]) check(`${text} on the active row`, rgb(text), active);
  expect(failures).toEqual([]);
});

it("keeps the nvim, VS Code and merged-PR tokens readable on surfaces and tints", () => {
  failures.length = 0;
  const active = over(token(light, "accent-soft"), rgb("bg-sidebar"));
  for (const kind of ["kind-nvim", "kind-vscode", "purple"]) {
    for (const name of surfaces) check(`${kind} on ${name}`, rgb(kind), rgb(name));
    check(`${kind} on the active row`, rgb(kind), active);
  }
  for (const name of ["surface", "bg-sidebar"]) {
    check(`purple on purple-soft/${name}`, rgb("purple"), over(token(light, "purple-soft"), rgb(name)));
  }
  expect(failures).toEqual([]);
});

it("faint text inside a surface-3 container resolves to text-muted", () => {
  failures.length = 0;
  const read = (f: string) => readFileSync(new URL(`../src/${f}`, import.meta.url), "utf8");
  const override = /([^{}]+)\{[^{}]*--text-faint:\s*var\(--text-muted\)[^{}]*\}/g;
  const scoped = (f: string) => [...read(f).matchAll(override)].map((m) => m[1]).join(",");
  const shared = scoped("styles.css");
  for (const sel of [".tree-chip", ".tag", ".chip", ".empty-icon", ".segmented", ".md code", ".md pre", ".nav-item:hover", ".tree-feature:hover", ".btn-ghost:hover", ".menu-item:hover"]) {
    expect(shared, sel).toContain(sel);
  }
  expect(scoped("sessions.css")).toContain(".custom-session-icon");
  expect(scoped("sessionSidebar.css")).toContain(".sb-chip");
  check("text-muted on surface-3", rgb("text-muted"), rgb("surface-3"));
  expect(failures).toEqual([]);
});
